use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub(crate) fn normalize_text(text: &str) -> String {
    text.nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_uppercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn addendum_rule_type(text: &str) -> &'static str {
    let text = normalize_text(text);
    if text.contains("ISOMEROS NAO LISTADOS NOMINALMENTE")
        && text.contains("MEDICAMENTOS REGISTRADOS")
    {
        "EXCLUDE_REGISTERED_MEDICINE_ISOMERS"
    } else if text.contains("ISOMEROS RELACIONADOS NOMINALMENTE EM OUTRA LISTA") {
        "EXCLUDE_NAMED_ISOMERS"
    } else if text.contains("SAIS")
        && (text.contains("SUBSTANCIAS ENUMERADAS ACIMA")
            || text.contains("SUBSTANCIAS DESTA LISTA"))
    {
        "INCLUDE_SALTS_AND_ISOMERS"
    } else {
        "TEXT_ONLY"
    }
}

fn contains_term(text: &str, term: &str) -> bool {
    !term.is_empty() && format!(" {text} ").contains(&format!(" {term} "))
}

fn prescription_kind(text: &str) -> Option<String> {
    let text = normalize_text(text);
    let words = text.split_whitespace().collect::<Vec<_>>();
    for pair in words.windows(2) {
        if pair[0] == "RECEITA" && matches!(pair[1], "A" | "B" | "B2") {
            return Some(pair[1].to_string());
        }
    }
    if text.contains("RECEITA DE CONTROLE ESPECIAL") {
        Some("CONTROL_SPECIAL".into())
    } else if text.contains("NOTIFICACAO DE RECEITA ESPECIAL") {
        Some("NOTICE_SPECIAL".into())
    } else {
        None
    }
}

struct ControlledList {
    code: String,
    prescription: Option<String>,
    names: BTreeSet<String>,
    derivative_rules: Vec<String>,
    exclusions: Vec<String>,
    exclude_registered_isomers: bool,
}

pub(crate) struct PortariaClassifier {
    lists: Vec<ControlledList>,
}

impl PortariaClassifier {
    pub(crate) fn new(lists: &[Value]) -> Self {
        let parenthetical = Regex::new(r"^([^()]+)\(([[:alpha:] -]{3,})\)$").unwrap();
        let lists = lists
            .iter()
            .filter(|list| {
                list["is_group"] != true && list["is_active"] != false && list["active"] != false
            })
            .map(|list| {
                let mut names = BTreeSet::new();
                for entry in list["entries"].as_array().into_iter().flatten() {
                    let name = entry["name"].as_str().unwrap_or_default();
                    if normalize_text(name).contains("EXCLUIDO") {
                        continue;
                    }
                    names.insert(normalize_text(name));
                    // Only a trailing, textual parenthesis is a name alias.
                    // Parentheses inside chemical formulas contain stereochemical
                    // markers such as (E), (S) and (2S), not substance names.
                    if let Some(alias) = parenthetical.captures(name) {
                        if !alias[1].contains(" ou ") {
                            names.insert(normalize_text(&alias[1]));
                            names.insert(normalize_text(&alias[2]));
                        }
                    }
                    for alias in entry["aliases"].as_array().into_iter().flatten() {
                        if let Some(alias) = alias.as_str() {
                            names.insert(normalize_text(alias));
                        }
                    }
                    for alias in name.split(" ou ") {
                        names.insert(normalize_text(alias));
                    }
                }
                names.remove("");
                let mut derivative_rules = Vec::new();
                let mut exclusions = Vec::new();
                let mut exclude_registered_isomers = false;
                for rule in list["addenda"].as_array().into_iter().flatten() {
                    let raw = rule["text"].as_str().unwrap_or_default();
                    let text = normalize_text(raw);
                    let kind = addendum_rule_type(raw);
                    if kind == "INCLUDE_SALTS_AND_ISOMERS" {
                        derivative_rules.push(text.clone());
                    }
                    if kind == "EXCLUDE_REGISTERED_MEDICINE_ISOMERS"
                        || rule["rule_type"] == "EXCLUDE_REGISTERED_MEDICINE_ISOMERS"
                    {
                        exclude_registered_isomers = true;
                    }
                    if text.contains("EXCETO")
                        || text.contains("EXCETUA")
                        || text.contains("EXCLUI")
                    {
                        exclusions.push(text);
                    }
                }
                ControlledList {
                    code: list["code"].as_str().unwrap_or_default().into(),
                    prescription: prescription_kind(
                        list["prescription_notice"].as_str().unwrap_or_default(),
                    ),
                    names,
                    derivative_rules,
                    exclusions,
                    exclude_registered_isomers,
                }
            })
            .collect();
        Self { lists }
    }

    pub(crate) fn classify(
        &self,
        prescriptions: &BTreeSet<String>,
        ingredients: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        if prescriptions.is_empty() {
            return BTreeSet::new();
        }
        let kinds = prescriptions
            .iter()
            .filter_map(|text| prescription_kind(text))
            .collect::<BTreeSet<_>>();
        let candidates = self
            .lists
            .iter()
            .filter(|list| {
                list.prescription
                    .as_ref()
                    .is_some_and(|kind| kinds.contains(kind))
            })
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return BTreeSet::from([candidates[0].code.clone()]);
        }
        // A recipe may apply to several lists, or differ because of an addendum
        // (e.g. tramadol in A2). Membership is then established by the substance.
        let ingredients = ingredients
            .iter()
            .map(|name| normalize_text(name))
            .collect::<BTreeSet<_>>();
        let exact_names = self
            .lists
            .iter()
            .flat_map(|list| &list.names)
            .collect::<BTreeSet<_>>();
        self.lists
            .iter()
            .filter(|list| {
                ingredients.iter().any(|ingredient| {
                    if list.names.contains(ingredient) {
                        return true;
                    }
                    // A substance named in another list takes precedence over a
                    // derivative match, including named isomer exceptions.
                    if exact_names.contains(ingredient) || list.derivative_rules.is_empty() {
                        return false;
                    }
                    if list
                        .exclusions
                        .iter()
                        .any(|text| contains_term(text, ingredient))
                    {
                        return false;
                    }
                    if list.exclude_registered_isomers
                        && (ingredient.contains("ISOMERO")
                            || ingredient
                                .split_whitespace()
                                .any(|word| matches!(word, "DEXTRO" | "LEVO" | "R" | "S")))
                    {
                        return false;
                    }
                    list.names
                        .iter()
                        .any(|name| contains_term(ingredient, name))
                })
            })
            .map(|list| list.code.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn list(code: &str, notice: &str, name: &str, addendum: &str) -> Value {
        json!({"code":code, "prescription_notice":notice, "entries":[{"name":name}],
            "addenda":[{"text":addendum}]})
    }

    #[test]
    fn prescription_precedes_substance_and_b2_does_not_match_b() {
        let classifier = PortariaClassifier::new(&[
            list("B1", "Notificação de Receita B", "diazepam", ""),
            list("B2", "Notificação de Receita B2", "sibutramina", ""),
        ]);
        assert_eq!(
            classifier.classify(
                &set(&["Venda sob Notificação de Receita \"B2\""]),
                &set(&["diazepam"])
            ),
            set(&["B2"])
        );
        assert_eq!(
            classifier.classify(&set(&["Venda sob Notificação de Receita \"B\""]), &set(&[])),
            set(&["B1"])
        );
    }

    #[test]
    fn ambiguous_prescription_and_generic_p_use_substances() {
        let classifier = PortariaClassifier::new(&[
            list("A1", "Notificação de Receita A", "morfina", ""),
            list(
                "A2",
                "Notificação de Receita A",
                "tramadol",
                "os sais das substâncias enumeradas acima",
            ),
        ]);
        assert_eq!(
            classifier.classify(&set(&["Notificação de Receita A"]), &set(&["tramadol"])),
            set(&["A2"])
        );
        assert_eq!(
            classifier.classify(
                &set(&["Venda sob Receita de Controle Especial"]),
                &set(&["cloridrato de tramadol"])
            ),
            set(&["A2"])
        );
        assert!(
            classifier
                .classify(&set(&["Notificação de Receita A"]), &set(&["desconhecida"]))
                .is_empty()
        );
        assert!(
            classifier
                .classify(&set(&[]), &set(&["tramadol"]))
                .is_empty()
        );
    }

    #[test]
    fn derivatives_require_inclusion_and_whole_names() {
        let classifier = PortariaClassifier::new(&[
            list(
                "C1",
                "",
                "sódio",
                "os sais e isômeros das substâncias enumeradas acima",
            ),
            list("C5", "", "testosterona", ""),
            list(
                "A1",
                "",
                "metadona",
                "os sais das substâncias enumeradas acima",
            ),
        ]);
        let prescription = set(&["Venda sob prescrição médica"]);
        assert_eq!(
            classifier.classify(&prescription, &set(&["Cloridrato de SÓDIO"])),
            set(&["C1"])
        );
        assert!(
            classifier
                .classify(
                    &prescription,
                    &set(&["éster de testosterona", "levometadona"])
                )
                .is_empty()
        );
        assert_eq!(
            classifier.classify(&prescription, &set(&["sódio", "testosterona"])),
            set(&["C1", "C5"])
        );
    }

    #[test]
    fn chemical_formula_fragments_are_not_substance_aliases() {
        let classifier = PortariaClassifier::new(&[
            list(
                "F1",
                "",
                "MITRAGININA ou METIL (E)-2-[(2S)-3-ETIL]",
                "os sais das substâncias enumeradas acima",
            ),
            list(
                "F2",
                "",
                "CATINONA ou (-)-(S)-2-AMINOPROPIOFENONA",
                "os sais das substâncias enumeradas acima",
            ),
            list(
                "A1",
                "",
                "Dimefeptanol (metadol)",
                "os sais das substâncias enumeradas acima",
            ),
        ]);
        let prescription = set(&["Prescrição médica"]);
        assert!(
            classifier
                .classify(
                    &prescription,
                    &set(&["vitamina E", "poliovírus tipo 2", "metil"])
                )
                .is_empty()
        );
        assert_eq!(
            classifier.classify(&prescription, &set(&["metadol"])),
            set(&["A1"])
        );
        assert_eq!(
            classifier.classify(&prescription, &set(&["mitraginina"])),
            set(&["F1"])
        );
    }

    #[test]
    fn analytical_standard_addenda_do_not_authorize_all_derivatives() {
        let classifier = PortariaClassifier::new(&[list(
            "C1",
            "",
            "exemplo",
            "padrões analíticos à base dos sais e isômeros das substâncias citadas",
        )]);
        assert!(
            classifier
                .classify(
                    &set(&["Prescrição médica"]),
                    &set(&["cloridrato de exemplo"])
                )
                .is_empty()
        );
    }

    #[test]
    fn explicit_exclusions_and_named_isomers_take_precedence() {
        let classifier = PortariaClassifier::new(&[
            list(
                "A1",
                "",
                "exemplo",
                "os sais e isômeros (exceto isômero de exemplo) das substâncias enumeradas acima",
            ),
            list("C1", "", "isômero de exemplo", ""),
        ]);
        assert_eq!(
            classifier.classify(&set(&["Prescrição médica"]), &set(&["isômero de exemplo"])),
            set(&["C1"])
        );
        let classifier = PortariaClassifier::new(&[list(
            "A1",
            "",
            "exemplo",
            "os sais e isômeros (exceto isômero de exemplo) das substâncias enumeradas acima",
        )]);
        assert!(
            classifier
                .classify(&set(&["Prescrição médica"]), &set(&["isômero de exemplo"]))
                .is_empty()
        );
    }
}

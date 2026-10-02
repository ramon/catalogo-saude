use crate::portaria::normalize_text;
use serde_json::Value;
use std::collections::BTreeSet;

fn has_substance(ingredients: &BTreeSet<String>, substance: &str) -> bool {
    ingredients.iter().any(|ingredient| {
        format!(" {} ", normalize_text(ingredient)).contains(&format!(" {substance} "))
    })
}

// Evidence is limited to dosage forms/routes, never indication or product names.
fn routes(forms: &BTreeSet<String>, presentations: &[Value]) -> (bool, bool) {
    let mut topical = false;
    let mut systemic = false;
    for form in forms.iter().map(String::as_str).chain(
        presentations
            .iter()
            .filter_map(|p| p["physical_form"].as_str()),
    ) {
        let form = normalize_text(form);
        topical |= form
            .split_whitespace()
            .any(|word| matches!(word, "CREME" | "GEL" | "POMADA"))
            || ["DERMATOLOG", "CUTAN", "TOPIC"]
                .iter()
                .any(|term| form.contains(term));
        systemic |= [
            "CAPSULA",
            "COMPRIMIDO",
            "INJETAVEL",
            "ORAL",
            "INTRAMUSCULAR",
            "INTRAVENOSA",
        ]
        .iter()
        .any(|term| form.contains(term));
    }
    (topical, systemic)
}

pub(crate) fn classify_prescriptions(
    restrictions: &BTreeSet<String>,
    lists: &BTreeSet<String>,
    ingredients: &BTreeSet<String>,
    forms: &BTreeSet<String>,
    presentations: &[Value],
) -> BTreeSet<&'static str> {
    if restrictions.is_empty() {
        return BTreeSet::new();
    }
    let has_list = |code| lists.iter().any(|list| list == code);
    let (topical, systemic) = routes(forms, presentations);
    // These medicines have a specific prescription outside the SNCR categories.
    if has_list("C3")
        && (has_substance(ingredients, "LENALIDOMIDA")
            || has_substance(ingredients, "POMALIDOMIDA"))
    {
        return BTreeSet::new();
    }
    if has_list("C3") && has_substance(ingredients, "TALIDOMIDA") {
        return BTreeSet::from(["NRT"]);
    }
    if has_list("C3") {
        return BTreeSet::new();
    }
    let b1_special = [
        "FENOBARBITAL",
        "METILFENOBARBITAL",
        "BARBITAL",
        "BARBEXACLONA",
        "PERAMPANEL",
    ]
    .iter()
    .any(|name| has_substance(ingredients, name));
    if has_list("B1") && b1_special && lists.len() == 1 {
        return BTreeSet::from(["RCE"]);
    }
    if has_list("C2") {
        return if systemic && !topical {
            BTreeSet::from(["NRR"])
        } else {
            BTreeSet::new()
        };
    }
    if has_list("C5") {
        if topical {
            return BTreeSet::new();
        }
        if systemic
            || restrictions
                .iter()
                .any(|text| normalize_text(text).contains("RECEITA DE CONTROLE ESPECIAL"))
        {
            return BTreeSet::from(["RCE"]);
        }
        return BTreeSet::new();
    }
    let mut explicit = BTreeSet::new();
    let mut retention = false;
    for restriction in restrictions {
        let text = normalize_text(restriction);
        let words = text.split_whitespace().collect::<Vec<_>>();
        for pair in words.windows(2) {
            if pair[0] == "RECEITA" {
                match pair[1] {
                    "A" => {
                        explicit.insert("NRA");
                    }
                    "B" => {
                        explicit.insert("NRB");
                    }
                    "B2" => {
                        explicit.insert("NRB2");
                    }
                    _ => {}
                }
            }
        }
        if text.contains("RECEITA DE CONTROLE ESPECIAL") {
            explicit.insert("RCE");
        }
        retention |= text.contains("RETENCAO DE RECEITA");
    }
    if !explicit.is_empty() {
        return explicit;
    }
    if lists.is_empty() {
        return if retention {
            BTreeSet::from(["RET"])
        } else {
            BTreeSet::new()
        };
    }
    let mut result = BTreeSet::new();
    for list in lists {
        match list.as_str() {
            // Dose/form-dependent addenda cannot be resolved from a substance name.
            "A1" if !has_substance(ingredients, "OXICODONA")
                && !has_substance(ingredients, "BUPRENORFINA")
                && !has_substance(ingredients, "DIFENOXILATO")
                && !has_substance(ingredients, "OPIO") =>
            {
                result.insert("NRA");
            }
            "A2" => {} // Preparations in the addenda can use RCE.
            "A3" => {
                result.insert("NRA");
            }
            "B1" => {
                let special = [
                    "FENOBARBITAL",
                    "METILFENOBARBITAL",
                    "BARBITAL",
                    "BARBEXACLONA",
                    "PERAMPANEL",
                ]
                .iter()
                .any(|name| has_substance(ingredients, name));
                result.insert(if special { "RCE" } else { "NRB" });
            }
            "B2" => {
                result.insert("NRB2");
            }
            "C1" => {
                result.insert("RCE");
            }
            "C5" if systemic && !topical => {
                result.insert("RCE");
            }
            _ => {} // C4, precursors and prohibited lists do not imply an SNCR recipe.
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn classify(
        restriction: &str,
        list: &str,
        ingredient: &str,
        form: &str,
    ) -> BTreeSet<&'static str> {
        classify_prescriptions(
            &BTreeSet::from([restriction.into()]),
            &if list.is_empty() {
                BTreeSet::new()
            } else {
                BTreeSet::from([list.into()])
            },
            &BTreeSet::from([ingredient.into()]),
            &BTreeSet::from([form.into()]),
            &[],
        )
    }
    #[test]
    fn distinguishes_retention_and_controlled_prescriptions() {
        assert_eq!(
            classify(
                "Venda sob prescrição médica com retenção de receita",
                "",
                "semaglutida",
                ""
            ),
            BTreeSet::from(["RET"])
        );
        assert_eq!(
            classify(
                "Venda Sob Receita de Controle Especial",
                "A2",
                "tramadol",
                ""
            ),
            BTreeSet::from(["RCE"])
        );
        assert_eq!(
            classify("Venda sob Prescrição Médica", "A2", "tramadol", ""),
            BTreeSet::new()
        );
        assert_eq!(
            classify("Notificação de Receita B2", "B2", "sibutramina", ""),
            BTreeSet::from(["NRB2"])
        );
        assert_eq!(
            classify("Venda sem Prescrição Médica", "", "", ""),
            BTreeSet::new()
        );
    }
    #[test]
    fn special_lists_require_substance_and_route_evidence() {
        assert_eq!(
            classify("Notificação de Receita A", "C3", "talidomida", ""),
            BTreeSet::from(["NRT"])
        );
        assert_eq!(
            classify(
                "Venda Sob Receita de Controle Especial",
                "C3",
                "lenalidomida",
                ""
            ),
            BTreeSet::new()
        );
        assert_eq!(
            classify(
                "Notificação de Receita A",
                "C2",
                "isotretinoína",
                "CAPSULA GELATINOSA DURA"
            ),
            BTreeSet::from(["NRR"])
        );
        assert_eq!(
            classify(
                "Notificação de Receita A",
                "C2",
                "isotretinoína",
                "Cápsula Mole"
            ),
            BTreeSet::from(["NRR"])
        );
        assert_eq!(
            classify("Notificação de Receita A", "C2", "isotretinoína", ""),
            BTreeSet::new()
        );
        assert_eq!(
            classify("Venda sob Prescrição Médica", "C2", "tretinoína", "Creme"),
            BTreeSet::new()
        );
        assert_eq!(
            classify("Venda sob Prescrição Médica", "C5", "testosterona", "Gel"),
            BTreeSet::new()
        );
        assert_eq!(
            classify(
                "Venda sob Prescrição Médica",
                "C5",
                "testosterona",
                "Solução Injetável"
            ),
            BTreeSet::from(["RCE"])
        );
        assert_eq!(
            classify("Venda sob Prescrição Médica", "B1", "fenobarbital", ""),
            BTreeSet::from(["RCE"])
        );
    }
}

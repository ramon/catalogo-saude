use crate::output::write_batches_iter;
use encoding_rs::WINDOWS_1252;
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

fn decode(bytes: &[u8]) -> String {
    WINDOWS_1252.decode(bytes).0.into_owned()
}
fn csv_rows(
    bytes: &[u8],
    delimiter: u8,
) -> Result<Vec<HashMap<String, String>>, Box<dyn std::error::Error>> {
    let decoded = decode(bytes).into_bytes();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(decoded.as_slice());
    let headers = reader
        .headers()?
        .iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    reader
        .records()
        .map(|result| {
            let rec = result?;
            Ok(headers
                .iter()
                .enumerate()
                .map(|(i, h)| (h.clone(), rec.get(i).unwrap_or("").trim().to_string()))
                .collect())
        })
        .collect()
}
pub(crate) fn get(row: &HashMap<String, String>, key: &str) -> String {
    row.get(key).cloned().unwrap_or_default()
}
fn medicine_source_identifier(row: &HashMap<String, String>) -> String {
    let process = get(row, "NU_PROCESSO");
    if !process.is_empty() {
        return format!("process:{}", normalized_id(&process));
    }
    let registration = get(row, "NU_REGISTRO_PRODUTO");
    if !registration.is_empty() {
        return format!("registration:{}", normalized_id(&registration));
    }
    let sequence = get(row, "CO_SEQ_PRODUTO");
    let normalize_text = |field: &str| {
        get(row, field)
            .to_uppercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "sequence:{sequence}:{}:{}:{}",
        normalize_text("NO_PRODUTO"),
        normalize_text("NO_RAZAO_SOCIAL_EMPRESA"),
        normalize_text("DS_TIPO_CATEGORIA_REGULATORIA")
    )
}
fn normalized_id(s: &str) -> String {
    let n = s.trim_start_matches('0');
    if n.is_empty() { "0".into() } else { n.into() }
}
fn null_if_empty(value: String) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        json!(value)
    }
}
fn first_nonempty(values: &[String]) -> String {
    values
        .iter()
        .find(|value| !value.is_empty())
        .cloned()
        .unwrap_or_default()
}
fn decimal_price(value: &str) -> Value {
    value
        .replace(".", "")
        .replace(",", ".")
        .parse::<f64>()
        .map(|n| json!(n))
        .unwrap_or(Value::Null)
}
fn date_br(value: &str) -> Value {
    let date = value.get(0..10).unwrap_or(value);
    chrono::NaiveDate::parse_from_str(date, "%d/%m/%Y")
        .map(|date| json!(date.to_string()))
        .unwrap_or(Value::Null)
}
struct PresentationDescriptionExpander {
    values: HashMap<String, String>,
    pattern: Regex,
}
impl PresentationDescriptionExpander {
    fn load(root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let values: HashMap<String, String> = serde_json::from_slice(&fs::read(
            root.join("references/presentation_abbreviations.json"),
        )?)?;
        let mut terms = values.keys().cloned().collect::<Vec<_>>();
        terms.sort_by_key(|term| std::cmp::Reverse(term.len()));
        let pattern = Regex::new(&format!(
            r"\b(?:{})\b",
            terms
                .iter()
                .map(|term| regex::escape(term))
                .collect::<Vec<_>>()
                .join("|")
        ))?;
        Ok(Self { values, pattern })
    }

    fn expand(&self, description: &str) -> String {
        self.pattern
            .replace_all(description, |matched: &regex::Captures<'_>| {
                let term = matched
                    .get(0)
                    .map(|value| value.as_str())
                    .unwrap_or_default();
                self.values
                    .get(term)
                    .map(String::as_str)
                    .unwrap_or(term)
                    .to_string()
            })
            .into_owned()
    }
}

fn manufacturer_parts(value: &str) -> (String, String) {
    let Some((candidate, name)) = value.split_once(" - ") else {
        return (String::new(), value.trim().to_string());
    };
    let digits = candidate
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    if digits.len() == 14 {
        (digits, name.trim().to_string())
    } else {
        (String::new(), value.trim().to_string())
    }
}
pub fn normalize_medicines(
    open: &[u8],
    consult: &[u8],
    prices: &[u8],
    root: &Path,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let open_rows = csv_rows(open, b';')?;
    let consult_rows = csv_rows(consult, b';')?;
    let price_rows = csv_rows(prices, b';')?;
    eprintln!(
        "Medicamentos: {} registros abertos, {} de consulta, {} preços",
        open_rows.len(),
        consult_rows.len(),
        price_rows.len()
    );
    let mut open_by_id: HashMap<String, Vec<HashMap<String, String>>> = HashMap::new();
    for r in open_rows {
        for k in ["NUMERO_PROCESSO", "NUMERO_REGISTRO_PRODUTO"] {
            let id = get(&r, k);
            if !id.is_empty() {
                open_by_id
                    .entry(normalized_id(&id))
                    .or_default()
                    .push(r.clone());
            }
        }
    }
    let mappings = load_anvisa_mappings(root)?;
    let description_expander = PresentationDescriptionExpander::load(root)?;
    let mut by_product: BTreeMap<String, Vec<HashMap<String, String>>> = BTreeMap::new();
    for r in consult_rows {
        let id = medicine_source_identifier(&r);
        by_product.entry(id).or_default().push(r);
    }
    eprintln!("Medicamentos: {} produtos agrupados", by_product.len());
    let registration_prefixes = by_product
        .values()
        .flatten()
        .map(|row| get(row, "NU_REGISTRO_PRODUTO"))
        .filter(|prefix| !prefix.is_empty())
        .collect::<BTreeSet<_>>();
    let prefix_lengths = registration_prefixes
        .iter()
        .map(String::len)
        .collect::<BTreeSet<_>>();
    let mut prices_by_prefix: HashMap<String, Vec<usize>> = HashMap::new();
    for (price_index, price) in price_rows.iter().enumerate() {
        let registration = get(price, "NU_REGISTRO");
        for prefix_length in &prefix_lengths {
            if let Some(prefix) = registration.get(..*prefix_length) {
                if registration_prefixes.contains(prefix) {
                    prices_by_prefix
                        .entry(prefix.to_string())
                        .or_default()
                        .push(price_index);
                }
            }
        }
    }
    eprintln!(
        "Medicamentos: índice de preços criado ({} prefixos) a partir de {} linhas",
        prices_by_prefix.len(),
        price_rows.len()
    );
    let mut out = Vec::new();
    let product_count = by_product.len();
    for (product_index, (source_id, rows)) in by_product.into_iter().enumerate() {
        if product_index == 0
            || (product_index + 1) % 5_000 == 0
            || product_index + 1 == product_count
        {
            eprintln!(
                "Medicamentos: conciliando produto {} de {}",
                product_index + 1,
                product_count
            );
        }
        let first = &rows[0];
        let process = get(first, "NU_PROCESSO");
        let reg = get(first, "NU_REGISTRO_PRODUTO");
        let matching = [process.as_str(), reg.as_str()]
            .iter()
            .filter(|id| !id.is_empty())
            .find_map(|id| open_by_id.get(&normalized_id(id)).and_then(|v| v.first()));
        let name = matching
            .map(|r| get(r, "NOME_PRODUTO"))
            .filter(|s| !s.is_empty())
            .unwrap_or(get(first, "NO_PRODUTO"));
        let mut forms = BTreeSet::new();
        let mut restrictions = BTreeSet::new();
        let mut restriction_types = BTreeSet::new();
        let mut tarjas = BTreeSet::new();
        let mut ingredients = BTreeSet::new();
        let mut synonyms = BTreeSet::new();
        for r in &rows {
            for (field, target) in [
                ("CO_FORMA_FISICA", &mut forms),
                ("CO_RESTRICAO", &mut restrictions),
                ("CO_TARJA", &mut tarjas),
                ("SINONIMOS", &mut synonyms),
                ("SUBSTANCIAS_MEDICAMENTOS", &mut ingredients),
            ] {
                for code in get(r, field)
                    .split([',', ';', '|', '+'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != "-")
                {
                    let val = if let Some(map) = mappings.get(field) {
                        map.get(code).cloned().unwrap_or_else(|| code.to_string())
                    } else {
                        code.to_string()
                    };
                    target.insert(val);
                }
            }
            for code in get(r, "CO_RESTRICAO")
                .split([',', ';', '|', '+'])
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "-")
            {
                if let Some(kind) = mappings.get("CO_RESTRICAO_TYPE").and_then(|m| m.get(code)) {
                    restriction_types.insert(kind.clone());
                }
            }
        }
        let mut presentations = Vec::new();
        let reg_prefixes = rows
            .iter()
            .map(|r| get(r, "NU_REGISTRO_PRODUTO"))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let matching_price_indices = reg_prefixes
            .iter()
            .filter_map(|prefix| prices_by_prefix.get(prefix))
            .flatten()
            .copied()
            .collect::<BTreeSet<_>>();
        for price_index in matching_price_indices {
            let p = &price_rows[price_index];
            let p_reg = get(p, "NU_REGISTRO");
            let description = get(p, "DS_APRESENTACAO");
            presentations.push(json!({"ggrem":null_if_empty(get(p,"CO_GGREM")),"ean":null_if_empty(get(p,"CO_EAN")),"registration_number":p_reg,"description":description,"expanded_description":description_expander.expand(&description),"active_ingredient":get(p,"DS_SUBSTANCIA"),"hospital_restricted":get(p,"ST_REST_HOSP"),"maximum_price":decimal_price(&get(p,"NU_PF18_INTEIRO"))}));
        }
        let raw_manufacturer = first_nonempty(&[
            get(matching.unwrap_or(first), "EMPRESA_DETENTORA_REGISTRO"),
            get(first, "NO_RAZAO_SOCIAL_EMPRESA"),
        ]);
        let (manufacturer_cnpj, manufacturer) = manufacturer_parts(&raw_manufacturer);
        out.push(json!({"source_identifier":source_id,"regulatory_identifier":first_nonempty(&[get(matching.unwrap_or(first),"NUMERO_PROCESSO"),get(matching.unwrap_or(first),"NUMERO_REGISTRO_PRODUTO"),process,reg]),"name":name,"manufacturer":manufacturer,"manufacturer_cnpj":manufacturer_cnpj,"regulatory_status":first_nonempty(&[get(matching.unwrap_or(first),"SITUACAO_REGISTRO"),get(first,"VALIDADE_SITUACAO")]),"regulatory_category":first_nonempty(&[get(matching.unwrap_or(first),"CATEGORIA_REGULATORIA"),get(first,"DS_TIPO_CATEGORIA_REGULATORIA")]),"reference":get(first,"DS_REFERENCIA"),"synonyms":synonyms,"indications":get(first,"INDICACOES"),"physical_forms":forms,"regulatory_restrictions":restrictions,"regulatory_restriction_types":restriction_types,"tarja":tarjas,"fractional_dispensing":get(first,"ST_DISPENSA_FRACIONADA").split([',',';','+']).map(str::trim).filter(|x|!x.is_empty()).collect::<Vec<_>>(),"active_ingredients":first_nonempty(&[get(first,"SUBSTANCIAS_MEDICAMENTOS"),get(matching.unwrap_or(first),"PRINCIPIO_ATIVO")]),"product_type":get(matching.unwrap_or(first),"TIPO_PRODUTO"),"presentations":presentations}));
    }
    eprintln!(
        "Medicamentos: normalização concluída ({} produtos)",
        out.len()
    );
    Ok(out)
}
fn load_anvisa_mappings(
    root: &Path,
) -> Result<HashMap<String, HashMap<String, String>>, Box<dyn std::error::Error>> {
    let mut all = HashMap::new();
    for (field, file) in [
        ("CO_FORMA_FISICA", "formas_fisicas.json"),
        ("CO_RESTRICAO", "restricao.json"),
        ("CO_TARJA", "tarja.json"),
    ] {
        let path = root.join("references/anvisa").join(file);
        let data: Vec<Value> = serde_json::from_slice(&fs::read(path)?)?;
        let map = data
            .iter()
            .filter_map(|v| {
                Some((
                    v.get("id")?.as_i64()?.to_string(),
                    v.get("descricao")?.as_str()?.trim().to_string(),
                ))
            })
            .collect();
        all.insert(field.into(), map);
        if file == "restricao.json" {
            let types = data
                .iter()
                .filter_map(|v| {
                    Some((
                        v.get("id")?.as_i64()?.to_string(),
                        v.get("tipoClassificacao")?.as_str()?.trim().to_string(),
                    ))
                })
                .collect();
            all.insert("CO_RESTRICAO_TYPE".into(), types);
        }
    }
    Ok(all)
}
pub fn normalize_cosmetics(
    bytes: &[u8],
    output: &Path,
    batch_size: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    let text = decode(bytes);
    let mut lines = text.lines();
    let header_line = lines.next().ok_or("CSV de cosméticos sem cabeçalho")?;
    let headers = parse_delimited_line(header_line)?;
    let mut columns = HashMap::new();
    for (index, header) in headers.iter().enumerate() {
        columns.insert(header.as_str(), index);
    }
    for required in [
        "NU_PROCESSO",
        "NO_PRODUTO",
        "NO_RAZAO_SOCIAL_EMPRESA",
        "ST_SITUACAO_PRODUTO",
        "NU_REGISTRO",
    ] {
        if !columns.contains_key(required) {
            return Err(format!("CSV de cosméticos sem coluna {required}").into());
        }
    }
    eprintln!("Cosméticos: processando linhas da fonte corrigindo aspas inválidas conhecidas");
    let mut by: HashMap<String, CosmeticRecord> = HashMap::new();
    let situation_index = columns["ST_SITUACAO_PRODUTO"];
    let mut row_count = 0usize;
    for (line_index, line) in lines.enumerate() {
        row_count += 1;
        if row_count % 100_000 == 0 {
            eprintln!("Cosméticos: processadas {row_count} linhas");
        }
        let line_number = line_index + 2;
        let mut values = parse_delimited_line(line).unwrap_or_default();
        if values.len() != headers.len()
            || !matches!(
                values.get(situation_index).map(String::as_str),
                Some("S" | "N")
            )
        {
            values = parse_delimited_line(&line.replacen("\"\";", "\"\"\";", 1))?;
        }
        if values.len() != headers.len() {
            return Err(format!(
                "linha {line_number} possui quantidade inválida de colunas em cosméticos"
            )
            .into());
        }
        let field = |name: &str| {
            values
                .get(columns[name])
                .cloned()
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let id = field("NU_PROCESSO");
        let name = field("NO_PRODUTO");
        if id.is_empty() || name.is_empty() {
            return Err(format!("cosmético sem processo/nome na linha {line_number}").into());
        };
        let status = field("ST_SITUACAO_PRODUTO");
        if status != "S" && status != "N" {
            return Err(format!("situação cosmético inválida na linha {line_number}").into());
        };
        let manufacturer = field("NO_RAZAO_SOCIAL_EMPRESA");
        let row = CosmeticRecord {
            registration_number: field("NU_REGISTRO"),
            name,
            manufacturer: if manufacturer == "-" {
                String::new()
            } else {
                manufacturer
            },
            regulatory_status: status.clone(),
        };
        if let Some(previous) = by.get_mut(&id) {
            if previous.registration_number != row.registration_number
                || previous.name != row.name
                || previous.manufacturer != row.manufacturer
            {
                return Err(format!("NU_PROCESSO {id} conflitante").into());
            }
            if status == "N" {
                previous.regulatory_status = "N".into();
            }
        } else {
            by.insert(id, row);
        }
    }
    eprintln!(
        "Cosméticos: {row_count} linhas lidas, {} produtos após deduplicação",
        by.len()
    );
    let rows = by.into_iter().map(|(id, row)| {
        json!({"source_identifier":id,"registration_number":row.registration_number,"name":row.name,"manufacturer":row.manufacturer,"regulatory_status":row.regulatory_status})
    });
    write_batches_iter(output, "cosmetics", rows, batch_size)
}
struct CosmeticRecord {
    registration_number: String,
    name: String,
    manufacturer: String,
    regulatory_status: String,
}
fn parse_delimited_line(line: &str) -> Result<Vec<String>, csv::Error> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .has_headers(false)
        .flexible(true)
        .from_reader(line.as_bytes());
    match reader.records().next() {
        Some(record) => record.map(|record| record.iter().map(str::to_string).collect()),
        None => Ok(Vec::new()),
    }
}
pub fn normalize_cannabis(bytes: &[u8]) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let rows = csv_rows(bytes, b';')?;
    eprintln!("Cannabis: {} linhas lidas", rows.len());
    let result = rows.iter().enumerate().map(|(i, r)| {
        let name = get(r, "NO_PRODUTO");
        let id = first_nonempty(&[
            get(r, "NU_PROCESSO"),
            get(r, "NU_REGISTRO_PRODUTO"),
            get(r, "CO_SEQ_PRODUTO"),
        ]);
        let reg = get(r, "NU_REGISTRO_PRODUTO");
        if name.is_empty() || id.is_empty() || reg.is_empty() {
            return Err(format!("produto de cannabis incompleto na linha {}", i + 2).into());
        }
        let category = get(r, "CO_TIPO_CAT_REGULATORIA");
        Ok(json!({"regulatory_code":id,"name":name,"manufacturer":get(r,"NO_RAZAO_SOCIAL_EMPRESA"),"regulatory_status":get(r,"SITUACAO_VALIDADE"),"active_ingredients":get(r,"PRINCIPIOS_ATIVOS"),"product_type":get(r,"CO_TIPO_PRODUTO"),"registration_number":reg,"regulatory_category":if category=="9"{"Produto de Cannabis"}else{&category},"source_updated_at":date_br(&get(r,"DT_ATUALIZACAO"))}))
    }).collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    eprintln!("Cannabis: {} produtos normalizados", result.len());
    Ok(result)
}
pub fn normalize_portaria(bytes: &[u8]) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let html = Html::parse_document(&decode(bytes));
    let selector = Selector::parse("p, tr")?;
    let blocks = html
        .select(&selector)
        .map(|e| {
            e.text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    eprintln!("Portaria 344: {} blocos HTML lidos", blocks.len());
    let header = Regex::new(r"(?i)^LISTA\s*[-–]?\s*(A[1-3]|B[1-2]|C[1-5]|D[1-2]|E|F[1-4]?)\b")?;
    let item = Regex::new(r"^(\d+(?:\.\d+)*)[.)]\s*(.*)$")?;
    let mut lists = Vec::new();
    let mut current: Option<Value> = None;
    for b in blocks {
        if let Some(c) = header.captures(&b) {
            if let Some(v) = current.take() {
                lists.push(v)
            }
            let code = c[1].to_uppercase();
            current = Some(
                json!({"code":code,"title":b,"label":format!("Lista {code}"),"prescription_notice":"","is_active":true,"is_group":code=="F","entries":[],"addenda":[]}),
            );
            continue;
        }
        if b.to_uppercase().starts_with("ANEXO II") {
            break;
        }
        if let Some(v) = current.as_mut() {
            if let Some(m) = item.captures(&b) {
                let n = m[1].to_string();
                let name = m[2].trim().to_string();
                if !name.is_empty() {
                    v["entries"].as_array_mut().unwrap().push(json!({"item_number":n,"name":name,"aliases":name.split(" ou ").skip(1).collect::<Vec<_>>(),"source_text":b,"kind":"SUBSTANCE"}));
                }
            } else if b.to_uppercase().starts_with("ADENDO") {
                v["addenda"].as_array_mut().unwrap().push(json!({"item_number":"","text":b,"rule_type":"TEXT_ONLY","target_substance_name":""}));
            }
        }
    }
    if let Some(v) = current {
        lists.push(v)
    }
    if lists.is_empty() {
        return Err("Portaria 344: Anexo I/listas não encontradas".into());
    };
    eprintln!("Portaria 344: {} listas normalizadas", lists.len());
    Ok(lists)
}

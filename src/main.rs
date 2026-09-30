use calamine::{Reader, Xlsx};
use chrono::Utc;
use clap::Parser;
use encoding_rs::WINDOWS_1252;
use regex::Regex;
use scraper::{Html, Selector};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use zip::ZipArchive;

const MED_OPEN: &str = "https://dados.anvisa.gov.br/dados/DADOS_ABERTOS_MEDICAMENTOS.csv";
const MED_CONSULT: &str =
    "https://dados.anvisa.gov.br/dados/CONSULTAS/PRODUTOS/TA_CONSULTA_MEDICAMENTOS.CSV";
const MED_PRICES: &str = "https://dados.anvisa.gov.br/dados/TA_PRECOS_MEDICAMENTOS.csv";
const COSMETICS: &str =
    "https://dados.anvisa.gov.br/dados/CONSULTAS/PRODUTOS/TA_CONSULTA_COSMETICOS.CSV";
const CANNABIS: &str =
    "https://dados.anvisa.gov.br/dados/CONSULTAS/PRODUTOS/TA_CONSULTA_PRODUTOS_CANNABIS.CSV";
const PORTARIA: &str = "https://anvisalegis.datalegis.net/action/ActionDatalegis.php?acao=abrirTextoAto&cod_menu=8542&cod_modulo=310&link=S&numeroAto=00000344&orgao=SVS%2FMS&seqAto=000&tipo=POR&valorAno=1998";
const SIGTAP: &str = "https://github.com/RenatoKR/SIGTAP/raw/refs/heads/main/tabelas/TabelaUnificada_202608_v2608141139.zip";
const ANS: &str = "https://www.gov.br/ans/pt-br/arquivos/assuntos/prestadores/padrao-para-troca-de-informacao-de-saude-suplementar-tiss/padrao-tiss-tabelas-relacionadas/padraotiss_mapeamento_tuss_sigtap.zip";

#[derive(Parser)]
#[command(about = "Baixa fontes de catálogos e grava lotes JSON normalizados")]
struct Args {
    #[arg(long, default_value = "./runs/current")]
    output: PathBuf,
    #[arg(long, default_value_t = 1000)]
    batch_size: usize,
}
#[derive(Serialize)]
struct SourceInfo {
    url: String,
    sha256: String,
    bytes: usize,
}
#[derive(Serialize)]
struct Manifest {
    schema_version: u8,
    generated_at: String,
    sources: BTreeMap<String, SourceInfo>,
    counts: BTreeMap<String, usize>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.batch_size == 0 {
        return Err("--batch-size deve ser maior que zero".into());
    }
    fs::create_dir_all(&args.output)?;
    let manifest_path = args.output.join("manifest.json");
    if manifest_path.exists() {
        fs::remove_file(&manifest_path)?;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .user_agent("catalog-data-loader/0.1")
        .build()?;
    let mut manifest = Manifest {
        schema_version: 1,
        generated_at: Utc::now().to_rfc3339(),
        sources: BTreeMap::new(),
        counts: BTreeMap::new(),
    };
    let med_open = fetch(&client, "medicines_open", MED_OPEN, &mut manifest)?;
    let med_consult = fetch(
        &client,
        "medicines_consultation",
        MED_CONSULT,
        &mut manifest,
    )?;
    let med_prices = fetch(&client, "medicines_prices", MED_PRICES, &mut manifest)?;
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let medicine_rows = normalize_medicines(&med_open, &med_consult, &med_prices, &project_root)?;
    manifest.counts.insert(
        "medicines".into(),
        write_batches(&args.output, "medicines", medicine_rows, args.batch_size)?,
    );

    let cosmetics = fetch(&client, "cosmetics", COSMETICS, &mut manifest)?;
    let cosmetic_rows = normalize_cosmetics(&cosmetics)?;
    manifest.counts.insert(
        "cosmetics".into(),
        write_batches(&args.output, "cosmetics", cosmetic_rows, args.batch_size)?,
    );
    let cannabis = fetch(&client, "cannabis", CANNABIS, &mut manifest)?;
    let cannabis_rows = normalize_cannabis(&cannabis)?;
    manifest.counts.insert(
        "cannabis".into(),
        write_batches(&args.output, "cannabis", cannabis_rows, args.batch_size)?,
    );

    let portaria = fetch(&client, "portaria_344", PORTARIA, &mut manifest)?;
    let portaria_rows = normalize_portaria(&portaria)?;
    manifest.counts.insert(
        "portaria_344_lists".into(),
        write_batches(
            &args.output,
            "portaria-344/lists",
            portaria_rows,
            args.batch_size,
        )?,
    );
    let sigtap = fetch(&client, "sigtap", SIGTAP, &mut manifest)?;
    let ans = fetch(&client, "ans_tuss_sigtap", ANS, &mut manifest)?;
    let (procedures, mappings, cids, cid_links) = normalize_sigtap(&sigtap, &ans)?;
    manifest.counts.insert(
        "diagnostic_procedures".into(),
        write_batches(
            &args.output,
            "sigtap/procedures",
            procedures,
            args.batch_size,
        )?,
    );
    manifest.counts.insert(
        "tuss_mappings".into(),
        write_batches(
            &args.output,
            "sigtap/tuss-mappings",
            mappings,
            args.batch_size,
        )?,
    );
    manifest.counts.insert(
        "cid10_codes".into(),
        write_batches(&args.output, "sigtap/cid10", cids, args.batch_size)?,
    );
    manifest.counts.insert(
        "procedure_cid10_links".into(),
        write_batches(
            &args.output,
            "sigtap/cid10-links",
            cid_links,
            args.batch_size,
        )?,
    );
    write_atomic(
        &args.output.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    println!(
        "Concluído: {} registros em {}",
        manifest.counts.values().sum::<usize>(),
        args.output.display()
    );
    Ok(())
}

fn fetch(
    client: &reqwest::blocking::Client,
    name: &str,
    url: &str,
    manifest: &mut Manifest,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    eprintln!("Baixando {name}: {url}");
    let response = client.get(url).send()?.error_for_status()?;
    let bytes = response.bytes()?.to_vec();
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    eprintln!("Recebido {name}: {} bytes, sha256 {digest}", bytes.len());
    manifest.sources.insert(
        name.into(),
        SourceInfo {
            url: url.into(),
            sha256: digest,
            bytes: bytes.len(),
        },
    );
    Ok(bytes)
}
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
fn get(row: &HashMap<String, String>, key: &str) -> String {
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
fn normalize_medicines(
    open: &[u8],
    consult: &[u8],
    prices: &[u8],
    root: &Path,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let open_rows = csv_rows(open, b';')?;
    let consult_rows = csv_rows(consult, b';')?;
    let price_rows = csv_rows(prices, b';')?;
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
    let mut by_product: BTreeMap<String, Vec<HashMap<String, String>>> = BTreeMap::new();
    for r in consult_rows {
        let id = medicine_source_identifier(&r);
        by_product.entry(id).or_default().push(r);
    }
    let mut out = Vec::new();
    for (source_id, rows) in by_product {
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
        for p in &price_rows {
            let p_reg = get(p, "NU_REGISTRO");
            if reg_prefixes.iter().any(|prefix| p_reg.starts_with(prefix)) {
                presentations.push(json!({"ggrem":null_if_empty(get(p,"CO_GGREM")),"ean":null_if_empty(get(p,"CO_EAN")),"registration_number":p_reg,"description":get(p,"DS_APRESENTACAO"),"active_ingredient":get(p,"DS_SUBSTANCIA"),"product_type":get(p,"TP_PRODUTO"),"hospital_restricted":get(p,"ST_REST_HOSP"),"minimum_price":decimal_price(&get(p,"NU_PF0_INTEIRO")),"maximum_price":decimal_price(&get(p,"NU_PF18_INTEIRO"))}));
            }
        }
        out.push(json!({"source_identifier":source_id,"regulatory_identifier":first_nonempty(&[get(matching.unwrap_or(first),"NUMERO_PROCESSO"),get(matching.unwrap_or(first),"NUMERO_REGISTRO_PRODUTO"),process,reg]),"name":name,"manufacturer":first_nonempty(&[get(matching.unwrap_or(first),"EMPRESA_DETENTORA_REGISTRO"),get(first,"NO_RAZAO_SOCIAL_EMPRESA")]),"regulatory_status":first_nonempty(&[get(matching.unwrap_or(first),"SITUACAO_REGISTRO"),get(first,"VALIDADE_SITUACAO")]),"regulatory_category":first_nonempty(&[get(matching.unwrap_or(first),"CATEGORIA_REGULATORIA"),get(first,"DS_TIPO_CATEGORIA_REGULATORIA")]),"reference":get(first,"DS_REFERENCIA"),"synonyms":synonyms,"indications":get(first,"INDICACOES"),"physical_forms":forms,"regulatory_restrictions":restrictions,"regulatory_restriction_types":restriction_types,"tarja":tarjas,"fractional_dispensing":get(first,"ST_DISPENSA_FRACIONADA").split([',',';','+']).map(str::trim).filter(|x|!x.is_empty()).collect::<Vec<_>>(),"active_ingredients":first_nonempty(&[get(first,"SUBSTANCIAS_MEDICAMENTOS"),get(matching.unwrap_or(first),"PRINCIPIO_ATIVO")]),"product_type":get(matching.unwrap_or(first),"TIPO_PRODUTO"),"presentations":presentations}));
    }
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
fn normalize_cosmetics(bytes: &[u8]) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let rows = csv_rows(bytes, b';')?;
    let mut by: BTreeMap<String, Value> = BTreeMap::new();
    for (i, r) in rows.iter().enumerate() {
        let id = get(r, "NU_PROCESSO");
        let name = get(r, "NO_PRODUTO");
        if id.is_empty() || name.is_empty() {
            return Err(format!("cosmético sem processo/nome na linha {}", i + 2).into());
        };
        let status = get(r, "ST_SITUACAO_PRODUTO");
        if status != "S" && status != "N" {
            return Err(format!("situação cosmético inválida na linha {}", i + 2).into());
        };
        let manufacturer = get(r, "NO_RAZAO_SOCIAL_EMPRESA");
        let row = json!({"source_identifier":id,"registration_number":get(r,"NU_REGISTRO"),"name":name,"manufacturer":if manufacturer=="-"{""}else{&manufacturer},"regulatory_status":status});
        if let Some(previous) = by.get_mut(&id) {
            for k in ["registration_number", "name", "manufacturer"] {
                if previous[k] != row[k] {
                    return Err(format!("NU_PROCESSO {id} conflitante").into());
                }
            }
            if status == "N" {
                previous["regulatory_status"] = json!("N");
            }
        } else {
            by.insert(id, row);
        }
    }
    Ok(by.into_values().collect())
}
fn normalize_cannabis(bytes: &[u8]) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let rows = csv_rows(bytes, b';')?;
    rows.iter().enumerate().map(|(i,r)|{let name=get(r,"NO_PRODUTO"); let id=first_nonempty(&[get(r,"NU_PROCESSO"),get(r,"NU_REGISTRO_PRODUTO"),get(r,"CO_SEQ_PRODUTO")]); let reg=get(r,"NU_REGISTRO_PRODUTO"); if name.is_empty()||id.is_empty()||reg.is_empty(){return Err(format!("produto de cannabis incompleto na linha {}",i+2).into())}; let category=get(r,"CO_TIPO_CAT_REGULATORIA"); Ok(json!({"regulatory_code":id,"name":name,"manufacturer":get(r,"NO_RAZAO_SOCIAL_EMPRESA"),"regulatory_status":get(r,"SITUACAO_VALIDADE"),"active_ingredients":get(r,"PRINCIPIOS_ATIVOS"),"product_type":get(r,"CO_TIPO_PRODUTO"),"registration_number":reg,"regulatory_category":if category=="9"{"Produto de Cannabis"}else{&category},"source_updated_at":date_br(&get(r,"DT_ATUALIZACAO"))}))}).collect()
}
fn normalize_portaria(bytes: &[u8]) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
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
    Ok(lists)
}

fn normalize_sigtap(
    bytes: &[u8],
    ans_bytes: &[u8],
) -> Result<(Vec<Value>, Vec<Value>, Vec<Value>, Vec<Value>), Box<dyn std::error::Error>> {
    let mut z = ZipArchive::new(Cursor::new(bytes))?;
    let names = (0..z.len())
        .map(|i| z.by_index(i).map(|f| f.name().to_string()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    for name in [
        "versao",
        "config.inf",
        "tb_procedimento.txt",
        "tb_procedimento_layout.txt",
        "tb_grupo.txt",
        "tb_grupo_layout.txt",
        "tb_sub_grupo.txt",
        "tb_sub_grupo_layout.txt",
        "tb_forma_organizacao.txt",
        "tb_forma_organizacao_layout.txt",
        "tb_cid.txt",
        "tb_cid_layout.txt",
        "rl_procedimento_cid.txt",
        "rl_procedimento_cid_layout.txt",
    ] {
        if !names.contains(name) {
            return Err(format!("ZIP SIGTAP sem {name}").into());
        }
    }
    let mut tables: HashMap<String, Vec<HashMap<String, String>>> = HashMap::new();
    let table_names = names
        .iter()
        .filter(|n| n.ends_with("_layout.txt"))
        .map(|n| n.trim_end_matches("_layout.txt").to_string())
        .collect::<Vec<_>>();
    for t in table_names {
        let layout = read_zip(&mut z, &format!("{t}_layout.txt"))?;
        let layout_text = String::from_utf8_lossy(&layout);
        let mut rdr = csv::Reader::from_reader(layout_text.as_bytes());
        let mut cols = Vec::new();
        for row in rdr.deserialize::<HashMap<String, String>>() {
            let r = row?;
            if let (Some(c), Some(a), Some(b)) = (r.get("Coluna"), r.get("Inicio"), r.get("Fim")) {
                cols.push((c.clone(), a.parse::<usize>()?, b.parse::<usize>()?));
            }
        }
        if cols.is_empty() {
            continue;
        }
        let data = read_zip(&mut z, &format!("{t}.txt"))?;
        let (decoded, _, _) = WINDOWS_1252.decode(&data);
        let mut rows = Vec::new();
        for line in decoded.lines().filter(|x| !x.is_empty()) {
            let mut row = HashMap::new();
            for (field, start, end) in &cols {
                row.insert(
                    field.clone(),
                    line.chars()
                        .skip(start - 1)
                        .take(end - start + 1)
                        .collect::<String>()
                        .trim()
                        .to_string(),
                );
            }
            rows.push(row)
        }
        tables.insert(t, rows);
    }
    let groups = index(&tables, "tb_grupo", "CO_GRUPO");
    let group = groups.get("02").ok_or("Grupo SIGTAP 02 ausente")?;
    if normalize_name(group.get("NO_GRUPO").map(String::as_str).unwrap_or(""))
        != "PROCEDIMENTOS COM FINALIDADE DIAGNOSTICA"
    {
        return Err("Grupo 02 não é diagnóstico".into());
    }
    let subgroups = index(&tables, "tb_sub_grupo", "CO_GRUPO");
    let organizations = index(&tables, "tb_forma_organizacao", "CO_GRUPO");
    let all = tables.get("tb_procedimento").cloned().unwrap_or_default();
    let mut procs = Vec::new();
    let mut codes = BTreeSet::new();
    for p in &all {
        let code = get(p, "CO_PROCEDIMENTO");
        if !code.starts_with("02") {
            continue;
        }
        codes.insert(code.clone());
        let subgroup = format!("{}{}", get(p, "CO_GRUPO"), get(p, "CO_SUB_GRUPO"));
        let org = format!(
            "{}{}{}",
            get(p, "CO_GRUPO"),
            get(p, "CO_SUB_GRUPO"),
            get(p, "CO_FORMA_ORGANIZACAO")
        );
        let sg = subgroups.get(&subgroup);
        let og = organizations.get(&org);
        procs.push(json!({"sigtap_code":code,"name":get(p,"NO_PROCEDIMENTO"),"group_code":"02","group_name":get(group,"NO_GRUPO"),"subgroup_code":subgroup,"subgroup_name":sg.map(|r|get(r,"NO_SUB_GRUPO")).unwrap_or_default(),"organization_code":org,"organization_name":og.map(|r|get(r,"NO_FORMA_ORGANIZACAO")).unwrap_or_default(),"tuss_codes":[],"areas":[],"occupations":[],"attributes":{"source_fields":p}}));
    }
    if procs.is_empty() {
        return Err("SIGTAP não contém procedimento diagnóstico".into());
    }
    procs.sort_by_key(|v| v["sigtap_code"].as_str().unwrap_or("").to_string());
    let cidrows = tables.get("tb_cid").cloned().unwrap_or_default();
    let mut cids = Vec::new();
    let mut cidcodes = BTreeSet::new();
    for c in cidrows {
        let code = get(&c, "CO_CID");
        if code.is_empty() {
            continue;
        }
        cidcodes.insert(code.clone());
        cids.push(json!({"code":code,"name":get(&c,"NO_CID"),"aggravation_type":get(&c,"TP_AGRAVO"),"sex":get(&c,"TP_SEXO"),"stage":get(&c,"TP_ESTADIO"),"irradiated_fields":get(&c,"VL_CAMPOS_IRRADIADOS")}));
    }
    let mut links = Vec::new();
    for r in tables
        .get("rl_procedimento_cid")
        .cloned()
        .unwrap_or_default()
    {
        let proc = get(&r, "CO_PROCEDIMENTO");
        let cid = get(&r, "CO_CID");
        if codes.contains(&proc) && cidcodes.contains(&cid) {
            links.push(json!({"sigtap_code":proc,"cid10_code":cid,"is_principal":get(&r,"ST_PRINCIPAL")=="S","competence":get(&r,"DT_COMPETENCIA")}));
        }
    }
    let tuss = index(&tables, "tb_tuss", "CO_TUSS");
    let mut mappings = Vec::new();
    for r in tables
        .get("rl_procedimento_tuss")
        .cloned()
        .unwrap_or_default()
    {
        let proc = get(&r, "CO_PROCEDIMENTO");
        let code = get(&r, "CO_TUSS");
        if codes.contains(&proc) {
            if let Some(t) = tuss.get(&code) {
                mappings.push(json!({"sigtap_code":proc,"tuss_code":code,"tuss_name":get(t,"NO_TUSS"),"source":"SIGTAP","source_revision":"","equivalence_grade":"","equivalence_label":"","mapping_status":"Relação publicada no SIGTAP","mapping_situation":""}));
            }
        }
    }
    mappings.extend(parse_ans_crosswalk(ans_bytes, &codes, &tuss)?);
    Ok((procs, mappings, cids, links))
}
fn parse_ans_crosswalk(
    bytes: &[u8],
    diagnostic: &BTreeSet<String>,
    tuss: &HashMap<String, HashMap<String, String>>,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut z = ZipArchive::new(Cursor::new(bytes))?;
    let names = (0..z.len())
        .map(|i| z.by_index(i).map(|f| f.name().to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let xlsx_name = names
        .iter()
        .find(|n| n.to_lowercase().ends_with(".xlsx"))
        .ok_or("ZIP ANS sem XLSX")?
        .clone();
    let workbook = read_zip(&mut z, &xlsx_name)?;
    let mut book: Xlsx<_> = Xlsx::new(Cursor::new(workbook))?;
    let range = book.worksheet_range("Mapeamento ativos")?;
    let mut headers = HashMap::new();
    for (i, v) in range.rows().next().unwrap_or(&[]).iter().enumerate() {
        headers.insert(v.to_string(), i);
    }
    let required = [
        "Código TUSS",
        "Termo TUSS",
        "Código Sigtap Final",
        "Status Final",
        "Grau de equivalencia",
        "Situação do Mapeamento",
    ];
    for h in required {
        if !headers.contains_key(h) {
            return Err(format!("ANS sem coluna {h}").into());
        }
    }
    let val = |row: &[calamine::Data], field: &str| -> String {
        headers
            .get(field)
            .and_then(|i| row.get(*i))
            .map(|v| v.to_string())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let grade_label = [
        ("1", "Equivalência lexical e conceitual"),
        ("2", "Equivalência conceitual com sinonímia"),
        ("3", "TUSS menos específico que SIGTAP"),
        ("4", "TUSS mais específico que SIGTAP"),
        ("5", "Não é possível mapear"),
    ]
    .into_iter()
    .collect::<HashMap<_, _>>();
    for row in range.rows().skip(1) {
        let sc = digits_padded(&val(row, "Código Sigtap Final"), 10);
        if !diagnostic.contains(&sc) || val(row, "Status Final") != "Mapeado" {
            continue;
        }
        let tc = digits_padded(&val(row, "Código TUSS"), 8);
        if !tuss.contains_key(&tc) {
            continue;
        }
        let grade = val(row, "Grau de equivalencia")
            .trim_end_matches(".0")
            .to_string();
        out.push(json!({"sigtap_code":sc,"tuss_code":tc,"tuss_name":val(row,"Termo TUSS"),"source":"ANS","source_revision":"","equivalence_grade":grade,"equivalence_label":grade_label.get(grade.as_str()).copied().unwrap_or(""),"mapping_status":"Mapeado","mapping_situation":val(row,"Situação do Mapeamento")}));
    }
    Ok(out)
}
fn digits_padded(raw: &str, width: usize) -> String {
    let s = raw.trim().trim_end_matches(".0");
    if !s.chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    format!("{s:0>width$}")
}
fn read_zip(
    z: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut f = z.by_name(name)?;
    let mut b = Vec::new();
    f.read_to_end(&mut b)?;
    Ok(b)
}
fn index(
    t: &HashMap<String, Vec<HashMap<String, String>>>,
    table: &str,
    key: &str,
) -> HashMap<String, HashMap<String, String>> {
    t.get(table)
        .into_iter()
        .flatten()
        .map(|r| (get(r, key), r.clone()))
        .collect()
}
fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_uppercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn null_if_empty(s: String) -> Value {
    if s.is_empty() { Value::Null } else { json!(s) }
}
fn first_nonempty(values: &[String]) -> String {
    values
        .iter()
        .find(|x| !x.is_empty())
        .cloned()
        .unwrap_or_default()
}
fn decimal_price(s: &str) -> Value {
    let value = s.replace(".", "").replace(",", ".");
    value
        .parse::<f64>()
        .map(|n| json!(n))
        .unwrap_or(Value::Null)
}
fn date_br(s: &str) -> Value {
    let d = s.get(0..10).unwrap_or(s);
    chrono::NaiveDate::parse_from_str(d, "%d/%m/%Y")
        .map(|x| json!(x.to_string()))
        .unwrap_or(Value::Null)
}
fn write_batches(
    root: &Path,
    name: &str,
    rows: Vec<Value>,
    batch_size: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("batch-") && name.ends_with(".json"))
        {
            fs::remove_file(path)?;
        }
    }
    let count = rows.len();
    for (i, chunk) in rows.chunks(batch_size).enumerate() {
        let file = dir.join(format!("batch-{:06}.json", i + 1));
        write_atomic(&file, &serde_json::to_vec_pretty(chunk)?)?;
    }
    Ok(count)
}
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut f = fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(tmp, path)?;
    Ok(())
}

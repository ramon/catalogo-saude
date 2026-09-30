use crate::anvisa::get;
use calamine::{Reader, Xlsx};
use encoding_rs::WINDOWS_1252;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    io::{Cursor, Read},
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use zip::ZipArchive;

pub fn normalize_sigtap(
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
    eprintln!(
        "SIGTAP: {} tabelas com layout para processar",
        table_names.len()
    );
    let table_count = table_names.len();
    for (table_index, t) in table_names.into_iter().enumerate() {
        eprintln!(
            "SIGTAP: lendo tabela {} de {} ({t})",
            table_index + 1,
            table_count
        );
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
        eprintln!("SIGTAP: tabela {t} contém {} linhas", rows.len());
        tables.insert(t, rows);
    }
    let groups = index(&tables, "tb_grupo", "CO_GRUPO");
    let group = groups.get("02").ok_or("Grupo SIGTAP 02 ausente")?;
    if normalize_name(group.get("NO_GRUPO").map(String::as_str).unwrap_or(""))
        != "PROCEDIMENTOS COM FINALIDADE DIAGNOSTICA"
    {
        return Err("Grupo 02 não é diagnóstico".into());
    }
    let subgroups = composite_index(&tables, "tb_sub_grupo", &["CO_GRUPO", "CO_SUB_GRUPO"]);
    let organizations = composite_index(
        &tables,
        "tb_forma_organizacao",
        &["CO_GRUPO", "CO_SUB_GRUPO", "CO_FORMA_ORGANIZACAO"],
    );
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
    eprintln!("SIGTAP: {} procedimentos diagnósticos", procs.len());
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
    eprintln!("CID-10: {} códigos", cids.len());
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
    eprintln!("CID-10: {} vínculos SIGTAP/CID", links.len());
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
    eprintln!("TUSS: {} vínculos SIGTAP e ANS", mappings.len());
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
        headers.insert(v.to_string().trim().to_string(), i);
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
fn composite_index(
    tables: &HashMap<String, Vec<HashMap<String, String>>>,
    table: &str,
    fields: &[&str],
) -> HashMap<String, HashMap<String, String>> {
    tables
        .get(table)
        .into_iter()
        .flatten()
        .map(|row| {
            let key = fields
                .iter()
                .map(|field| get(row, field))
                .collect::<String>();
            (key, row.clone())
        })
        .collect()
}
fn normalize_name(s: &str) -> String {
    s.nfkd()
        .filter(|character| !is_combining_mark(*character))
        .filter(|c| c.is_ascii_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_uppercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

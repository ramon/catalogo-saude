use crate::downloads::{DownloadControl, Manifest, save_source_bytes};
use calamine::{Data, Reader, open_workbook_auto};
use chrono::{Duration as ChronoDuration, NaiveDate};
use rand::{Rng, distr::Alphanumeric};
use regex::Regex;
use reqwest::cookie::CookieStore;
use scraper::{Html, Selector};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    time::{Duration, Instant},
};
use tungstenite::{
    Message, client::IntoClientRequest, handshake::client::Request, stream::MaybeTlsStream,
};

pub const PAGE: &str = "https://mapa-indicadores.agricultura.gov.br/publico/extensions/Produtos_Farmaceuticos/Produtos_Farmaceuticos.html";
const APP_ID: &str = "ab6f1c23-f41e-4977-8457-8d2a1d87054a";
const WS_URL: &str =
    "wss://mapa-indicadores.agricultura.gov.br/publico/app/ab6f1c23-f41e-4977-8457-8d2a1d87054a";
const TIMEOUT: Duration = Duration::from_secs(60);
const CONTROLLED_LIST_CODES: &[&str] = &["A1", "A2", "A3", "B1", "C1", "C2", "C5"];

pub fn normalize_controlled_substances(
    bytes: &[u8],
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let html = Html::parse_document(&String::from_utf8_lossy(bytes));
    let selector = Selector::parse("p, tr, li, h1, h2, h3, h4")?;
    let header = Regex::new(r"(?i)^LISTA\s+([A-Z]\d+)\s*:\s*(.+)$")?;
    let mut in_annex = false;
    let mut current: Option<(String, String, BTreeSet<String>)> = None;
    let mut lists = Vec::new();

    for element in html.select(&selector) {
        let text = element
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        let normalized = crate::portaria::normalize_text(&text);
        if normalized == "ANEXO I" {
            in_annex = true;
            continue;
        }
        if normalized.starts_with("ANEXO II") {
            break;
        }
        if !in_annex {
            continue;
        }
        if let Some(captures) = header.captures(&text) {
            if let Some((code, title, names)) = current.take() {
                lists.push(json!({
                    "code": code,
                    "title": title,
                    "entries": names.into_iter().map(|name| json!({"name": name})).collect::<Vec<_>>(),
                }));
            }
            current = Some((
                captures[1].to_uppercase(),
                captures[2].trim().into(),
                BTreeSet::new(),
            ));
            continue;
        }
        if let Some((_, _, names)) = current.as_mut() {
            if !normalized
                .chars()
                .all(|character| character.is_ascii_digit())
            {
                names.insert(text);
            }
        }
    }
    if let Some((code, title, names)) = current {
        lists.push(json!({
            "code": code,
            "title": title,
            "entries": names.into_iter().map(|name| json!({"name": name})).collect::<Vec<_>>(),
        }));
    }

    let found = lists
        .iter()
        .filter_map(|list| list["code"].as_str())
        .collect::<BTreeSet<_>>();
    let missing = CONTROLLED_LIST_CODES
        .iter()
        .filter(|code| !found.contains(**code))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "Anexo I da Portaria MAPA nº 837/2025 incompleto; listas não encontradas: {}",
            missing.join(", ")
        )
        .into());
    }
    Ok(lists)
}

struct MapaControlledClassifier {
    lists: Vec<(String, BTreeSet<String>)>,
}

impl MapaControlledClassifier {
    fn new(lists: &[Value]) -> Self {
        let lists = lists
            .iter()
            .filter_map(|list| {
                let code = list["code"].as_str()?.to_owned();
                let names = list["entries"]
                    .as_array()?
                    .iter()
                    .filter_map(|entry| entry["name"].as_str())
                    .map(crate::portaria::normalize_text)
                    .filter(|name| !name.is_empty())
                    .collect();
                Some((code, names))
            })
            .collect();
        Self { lists }
    }

    fn classify(&self, ingredients: &[String]) -> Vec<String> {
        let ingredients = ingredients
            .iter()
            .map(|name| crate::portaria::normalize_text(name))
            .filter(|name| !name.is_empty())
            .collect::<BTreeSet<_>>();
        self.lists
            .iter()
            .filter(|(_, names)| {
                ingredients.iter().any(|ingredient| {
                    names.iter().any(|name| {
                        contains_term(ingredient, name) || contains_term(name, ingredient)
                    })
                })
            })
            .map(|(code, _)| code.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

fn contains_term(text: &str, term: &str) -> bool {
    !term.is_empty() && format!(" {text} ").contains(&format!(" {term} "))
}

pub fn collect(
    client: &reqwest::blocking::Client,
    jar: &reqwest::cookie::Jar,
    controlled_lists: &[Value],
    output: &Path,
    manifest: &mut Manifest,
    download_control: &mut DownloadControl,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let page_url = reqwest::Url::parse(PAGE)?;
    let response = client.get(page_url.clone()).send()?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        let preview = body.chars().take(240).collect::<String>();
        return Err(format!("Página MAPA respondeu HTTP {status}: {preview}").into());
    }
    let header_csrf = response
        .headers()
        .get("qlik-csrf-token")
        .or_else(|| response.headers().get("x-qlik-csrf-token"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let html = response.text()?;
    if html.trim().is_empty() {
        return Err("Página pública do MAPA retornou HTML vazio".into());
    }

    let cookie_csrf = jar
        .cookies(&page_url)
        .and_then(|value| value.to_str().ok().map(str::to_owned))
        .as_deref()
        .and_then(csrf_cookie)
        .map(str::to_owned);
    let csrf = match header_csrf.or(cookie_csrf) {
        Some(token) => token,
        None => request_csrf_token(client, &page_url)?,
    };
    let cookie_header = jar
        .cookies(&page_url)
        .and_then(|value| value.to_str().ok().map(str::to_owned));
    let mut ws_url = reqwest::Url::parse(WS_URL)?;
    ws_url
        .query_pairs_mut()
        .append_pair("qlik-csrf-token", &csrf);
    let mut request: Request = ws_url.as_str().into_client_request()?;
    request.headers_mut().insert(
        "origin",
        "https://mapa-indicadores.agricultura.gov.br".parse()?,
    );
    if let Some(cookies) = cookie_header {
        request.headers_mut().insert("cookie", cookies.parse()?);
    }
    let (mut socket, _) = connect_websocket(request)?;
    let mut rpc = QlikRpc::new();
    let app = rpc.call(
        &mut socket,
        -1,
        "OpenDoc",
        json!([APP_ID, "", "", "", false]),
    )?;
    let app_handle = returned_handle(&app)?;
    let field = rpc.call(
        &mut socket,
        app_handle,
        "GetField",
        json!(["Produto_tabela1", null]),
    )?;
    let field_handle = returned_handle(&field)?;
    rpc.call(&mut socket, field_handle, "SelectAll", json!([false]))?;
    let table = rpc.call(&mut socket, app_handle, "GetObject", json!(["pNLmp"]))?;
    let table_handle = returned_handle(&table)?;
    let export = rpc.call(
        &mut socket,
        table_handle,
        "ExportData",
        json!(["OOXML", "/qHyperCubeDef", "", "P"]),
    )?;
    let q_url = find_string(&export, "qUrl").ok_or("ExportData do Qlik não retornou qUrl")?;
    let download_url = export_url(q_url)?;
    let download_started = Instant::now();
    let response = client.get(download_url.clone()).timeout(TIMEOUT).send()?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        let preview = body.chars().take(240).collect::<String>();
        return Err(format!("Download do MAPA respondeu HTTP {status}: {preview}").into());
    }
    let bytes = response.bytes()?;
    let download_elapsed_ms = download_started.elapsed().as_millis();
    validate_xlsx(&bytes)?;
    let path = save_source_bytes(
        "mapa_veterinary_pharmaceutical_products",
        "xlsx",
        PAGE,
        &bytes,
        download_elapsed_ms,
        output,
        manifest,
        download_control,
    )?;
    let raw_rows = read_xlsx(&path)?;
    if raw_rows.is_empty() {
        return Err(
            "O XLSX de produtos farmacêuticos veterinários do MAPA não contém registros".into(),
        );
    }
    let rows = normalize_products(raw_rows, controlled_lists)?;
    eprintln!(
        "MAPA: {} registros em {:.3}s",
        rows.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(rows)
}

fn normalize_products(
    rows: Vec<Value>,
    controlled_lists: &[Value],
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let classifier = MapaControlledClassifier::new(controlled_lists);
    rows.into_iter()
        .map(|row| {
            let registration = field(&row, "Registro do Produto");
            let product_name = field(&row, "Nome do Produto");
            if registration.is_empty() || product_name.is_empty() {
                return Err(format!(
                    "Produto MAPA sem nome ou registro: nome={product_name:?}, registro={registration:?}"
                )
                .into());
            }

            let presentation_text = field(&row, "Apresentação do Produto");
            let pharmaceutical_form = null_if_placeholder(field(&row, "Forma Farmacêutica"));
            let active_ingredients = split_values(field(&row, "IFA´s"));
            let controlled_substance_lists = classifier.classify(&active_ingredients);
            let presentations = split_presentations(&presentation_text)
                .into_iter()
                .map(|description| {
                    json!({
                        "concentration": null,
                        "physical_form": pharmaceutical_form,
                        "ggrem": null,
                        "ean": null,
                        "registration_number": registration,
                        "description": description,
                        "expanded_description": description,
                        "active_ingredient": active_ingredients.join(", "),
                        "hospital_restricted": null,
                        "maximum_price": null,
                    })
                })
                .collect::<Vec<_>>();

            let therapeutic_classes = split_values(field(&row, "Classe Farmacêutica"));
            let physical_forms = pharmaceutical_form
                .map(|value| vec![value.to_owned()])
                .unwrap_or_default();
            let manufacturer_cnpj = null_if_placeholder(field(&row, "CPF/CNPJ"));
            let manufacturer = null_if_placeholder(field(&row, "Nome  do Estabelecimento"));
            let status = null_if_placeholder(field(&row, "Situação do registro"));
            let indications = null_if_placeholder(field(&row, "Indicação"));
            let product_type = null_if_placeholder(field(&row, "Classificação do estabelecimento"));

            let veterinary_details = json!({
                "species": split_values(field(&row, "Espécie Animal")),
                "administration_route": null_if_placeholder(field(&row, "Via Administração")),
                "warnings": null_if_placeholder(field(&row, "Advertência")),
                "manufacturer_name": null_if_placeholder(field(&row, "Nome Fabricante")),
                "registration_holder": manufacturer,
                "establishment_registration": null_if_placeholder(field(&row, "Registro do Estabelecimento")),
                "previous_registration": null_if_placeholder(field(&row, "Registro Anterior")),
                "additional_characteristic": null_if_placeholder(field(&row, "Característica Adicional")),
                "raw_regulatory_category": null_if_placeholder(field(&row, "Classificação do estabelecimento")),
                "registration_granted_date": excel_date(field(&row, "DT Concessão Registro")),
                "previous_registration_granted_date": excel_date(field(&row, "DT Concessão do Registro Anterior")),
                "registration_expiry_date": excel_date(field(&row, "DT Vencimento Registro")),
                "product_validity_months": null_if_placeholder(field(&row, "Validade do Produto")),
                "origin": null_if_placeholder(field(&row, "Origem")),
                "country_of_production": null_if_placeholder(field(&row, "País de Produção")),
                "importer": null_if_placeholder(field(&row, "Importador")),
                "importer_address": null_if_placeholder(field(&row, "Endereço do Importador")),
                "establishment_activities": split_values(field(&row, "Grupo Atividade")),
                "raw_material_groups": split_values(field(&row, "Grupo Matéria Prima")),
                "homeopathic_inputs": null_if_placeholder(field(&row, "Insumos Homeopáticos")),
            });

            Ok(json!({
                "source_identifier": format!("mapa_veterinary:{registration}"),
                "regulatory_identifier": registration,
                "name": product_name,
                "manufacturer": manufacturer,
                "manufacturer_cnpj": manufacturer_cnpj,
                "regulatory_status": status,
                "therapeutic_classes": therapeutic_classes,
                "regulatory_category": null,
                "reference": null,
                "synonyms": [],
                "indications": indications,
                "directions": null_if_placeholder(field(&row, "Modo de Uso")),
                "physical_forms": physical_forms,
                "regulatory_restrictions": [],
                "regulatory_restriction_types": [],
                "portaria_344_lists": [],
                "mapa_controlled_substance_lists": controlled_substance_lists,
                "sncr_prescription_types": [],
                "tarja": [],
                "fractional_dispensing": [],
                "active_ingredients": active_ingredients,
                "product_type": product_type,
                "presentations": presentations,
                "veterinary_details": veterinary_details,
            }))
        })
        .collect()
}

fn field<'a>(row: &'a Value, name: &str) -> &'a str {
    row.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
}

fn null_if_placeholder(value: &str) -> Option<&str> {
    (!value.is_empty() && value != "-").then_some(value)
}

fn split_values(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|part| !part.is_empty() && *part != "-")
        .map(str::to_owned)
        .collect()
}

fn split_presentations(value: &str) -> Vec<String> {
    const STARTS: &[&str] = &[
        "AMPOLA",
        "AMPOLA(S)",
        "BAGS",
        "BALDE",
        "BARRICA",
        "BIG",
        "BISNAGA",
        "BISNAGA(S)",
        "BOLSA",
        "BOLSA(S)",
        "BOMBONA",
        "BOTIJÃO",
        "CAIXA",
        "CAIXA(S)",
        "CAPSULA(S)",
        "CARTUCHO(S)",
        "COMPRIMIDO(S)",
        "DISPLAY",
        "DISPOSITIVO(S)",
        "ENVELOPE",
        "FIBROLATA",
        "FLACONETE",
        "FRASCO",
        "FRASCO-AMPOLA",
        "FRASCO(S)",
        "FRASCO(S)-AMPOLA",
        "GALÃO",
        "GALAO",
        "GARRAFA",
        "GARRAFÃO",
        "LATA",
        "OUTROS",
        "PIPETA",
        "POTE",
        "SABONETE(S)",
        "SACO",
        "SACO(S)",
        "SACHE",
        "SACHÊ(S)",
        "SERINGA",
        "SERINGA(S)",
        "SPRAY",
        "STRIP",
        "TAMBOR",
        "TABLETE",
        "TUBO",
    ];
    if value.trim().is_empty() || value.trim() == "-" {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, character) in value.char_indices() {
        if character != ',' {
            continue;
        }
        let next = value[index + character.len_utf8()..].trim_start();
        if STARTS.iter().any(|prefix| next.starts_with(prefix)) {
            let part = value[start..index].trim();
            if !part.is_empty() {
                parts.push(part.to_owned());
            }
            start = index + character.len_utf8();
        }
    }
    let tail = value[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_owned());
    }
    parts
}

fn excel_date(value: &str) -> Option<String> {
    let serial = value.parse::<f64>().ok()?;
    if !serial.is_finite() || serial < 0.0 {
        return None;
    }
    let whole_days = serial.floor() as i64;
    NaiveDate::from_ymd_opt(1899, 12, 30)?
        .checked_add_signed(ChronoDuration::days(whole_days))
        .map(|date| date.format("%Y-%m-%d").to_string())
}

fn connect_websocket(
    request: Request,
) -> Result<
    (
        tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
        tungstenite::handshake::client::Response,
    ),
    Box<dyn std::error::Error>,
> {
    let host = request
        .uri()
        .host()
        .ok_or("URL do WebSocket sem hostname")?;
    let port = request.uri().port_u16().unwrap_or(443);
    let addresses = (host, port).to_socket_addrs()?;
    let mut last_error = None;
    let mut stream = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, Duration::from_secs(30)) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let stream = stream.ok_or_else(|| {
        last_error.unwrap_or_else(|| std::io::Error::other("hostname não resolveu endereços"))
    })?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    tungstenite::client_tls_with_config(request, stream, None, None)
        .map_err(|error| format!("Handshake WebSocket do MAPA falhou: {error:?}").into())
}

fn csrf_cookie(header: &str) -> Option<&str> {
    header.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        matches!(
            name.to_ascii_lowercase().as_str(),
            "qlik-csrf-token" | "csrf-token"
        )
        .then_some(value)
    })
}

fn request_csrf_token(
    client: &reqwest::blocking::Client,
    page_url: &reqwest::Url,
) -> Result<String, Box<dyn std::error::Error>> {
    let xrfkey = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(16)
        .map(char::from)
        .collect::<String>();
    let mut token_url = page_url.clone();
    token_url.set_path("/publico/qps/csrftoken");
    token_url.query_pairs_mut().append_pair("Xrfkey", &xrfkey);
    let response = client
        .get(token_url)
        .header("X-Qlik-Xrfkey", &xrfkey)
        .send()?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        let preview = body.chars().take(240).collect::<String>();
        return Err(format!("Endpoint CSRF do MAPA respondeu HTTP {status}: {preview}").into());
    }
    response
        .headers()
        .get("qlik-csrf-token")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| "Endpoint CSRF do MAPA não retornou qlik-csrf-token".into())
}

struct QlikRpc {
    next_id: u64,
}

impl QlikRpc {
    fn new() -> Self {
        Self { next_id: 1 }
    }

    fn call(
        &mut self,
        socket: &mut tungstenite::WebSocket<MaybeTlsStream<std::net::TcpStream>>,
        handle: i64,
        method: &str,
        params: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "handle": handle,
            "method": method,
            "params": params,
        });
        socket.send(Message::Text(request.to_string().into()))?;
        loop {
            let message = socket.read()?;
            match message {
                Message::Text(text) => {
                    let response: Value = serde_json::from_str(&text)?;
                    if response.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                        return Err(format!("Resposta JSON-RPC inválida para {method}").into());
                    }
                    if let Some(error) = response.get("error") {
                        return Err(format!("Erro JSON-RPC em {method}: {error}").into());
                    }
                    let result = response
                        .get("result")
                        .ok_or_else(|| format!("Resposta sem result para {method}"))?;
                    check_warnings(result, method)?;
                    return Ok(result.clone());
                }
                Message::Ping(data) => socket.send(Message::Pong(data))?,
                Message::Close(frame) => {
                    return Err(
                        format!("WebSocket do Qlik fechou durante {method}: {frame:?}").into(),
                    );
                }
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }
}

fn check_warnings(value: &Value, method: &str) -> Result<(), Box<dyn std::error::Error>> {
    match value {
        Value::Object(object) => {
            if let Some(warnings) = object.get("qWarnings") {
                let has_warning = warnings.as_array().is_some_and(|items| !items.is_empty())
                    || (!warnings.is_null() && !warnings.as_array().is_some());
                if has_warning {
                    return Err(format!("Qlik retornou qWarnings em {method}: {warnings}").into());
                }
            }
            for child in object.values() {
                check_warnings(child, method)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                check_warnings(child, method)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn returned_handle(value: &Value) -> Result<i64, Box<dyn std::error::Error>> {
    find_integer(value, "qHandle").ok_or_else(|| "Resposta Qlik sem qHandle".into())
}

fn find_integer(value: &Value, key: &str) -> Option<i64> {
    match value {
        Value::Object(object) => object
            .get(key)
            .and_then(Value::as_i64)
            .or_else(|| object.values().find_map(|child| find_integer(child, key))),
        Value::Array(items) => items.iter().find_map(|child| find_integer(child, key)),
        _ => None,
    }
}

fn find_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Object(object) => object
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| object.values().find_map(|child| find_string(child, key))),
        Value::Array(items) => items.iter().find_map(|child| find_string(child, key)),
        _ => None,
    }
}

fn export_url(q_url: &str) -> Result<reqwest::Url, Box<dyn std::error::Error>> {
    let origin = reqwest::Url::parse("https://mapa-indicadores.agricultura.gov.br/")?;
    let (path, query) = q_url.split_once('?').unwrap_or((q_url, ""));
    let mut path_url = if q_url.starts_with("https://") {
        reqwest::Url::parse(q_url)?
    } else {
        let path = path.trim_start_matches('/');
        let path = if path.starts_with("publico/") {
            format!("/{path}")
        } else {
            format!("/publico/{path}")
        };
        let mut url = origin.clone();
        url.set_path(&path);
        if !query.is_empty() {
            url.set_query(Some(query));
        }
        url
    };
    if path_url.host_str() != origin.host_str() || path_url.scheme() != "https" {
        return Err("qUrl do Qlik aponta para uma origem não confiável".into());
    }
    if !path_url.path().starts_with("/publico/") {
        let rewritten = format!("/publico{}", path_url.path());
        path_url.set_path(&rewritten);
    }
    Ok(path_url)
}

fn validate_xlsx(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(
        bytes.get(..4),
        Some(b"PK\x03\x04" | b"PK\x05\x06" | b"PK\x07\x08")
    ) {
        let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(160)]);
        return Err(format!("ExportData não retornou XLSX válido (início: {preview:?})").into());
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    if archive.by_name("xl/workbook.xml").is_err() {
        return Err("Arquivo ZIP recebido não contém xl/workbook.xml".into());
    }
    Ok(())
}

fn read_xlsx(path: &Path) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut workbook = open_workbook_auto(path)?;
    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or("XLSX do MAPA não contém planilhas")?;
    let range = workbook.worksheet_range(&sheet_name)?;
    let mut rows = range.rows();
    let header_row = rows.next().ok_or("Planilha do MAPA está vazia")?;
    let mut used = BTreeSet::new();
    let headers = header_row
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let header = value.to_string().trim().to_owned();
            let base = if header.is_empty() {
                format!("column_{}", index + 1)
            } else {
                header
            };
            let mut unique = base.clone();
            let mut suffix = 2;
            while !used.insert(unique.clone()) {
                unique = format!("{base}_{suffix}");
                suffix += 1;
            }
            unique
        })
        .collect::<Vec<_>>();
    let output = rows
        .filter(|row| row.iter().any(|cell| !matches!(cell, Data::Empty)))
        .map(|row| {
            let mut object = serde_json::Map::new();
            for (index, header) in headers.iter().enumerate() {
                object.insert(
                    header.clone(),
                    row.get(index).map(data_value).unwrap_or(Value::Null),
                );
            }
            Value::Object(object)
        })
        .collect();
    Ok(output)
}

fn data_value(value: &Data) -> Value {
    match value {
        Data::Empty => Value::Null,
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => {
            Value::String(value.clone())
        }
        Data::Int(value) => json!(value),
        Data::Float(value) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Data::Bool(value) => json!(value),
        Data::DateTime(value) => Value::String(value.to_string()),
        Data::Error(value) => Value::String(format!("{value:?}")),
    }
}

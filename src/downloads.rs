use crate::output::write_atomic;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{path::Path, time::Instant};

pub const MED_OPEN: &str = "https://dados.anvisa.gov.br/dados/DADOS_ABERTOS_MEDICAMENTOS.csv";
pub const MED_CONSULT: &str =
    "https://dados.anvisa.gov.br/dados/CONSULTAS/PRODUTOS/TA_CONSULTA_MEDICAMENTOS.CSV";
pub const MED_PRICES: &str = "https://dados.anvisa.gov.br/dados/TA_PRECOS_MEDICAMENTOS.csv";
pub const COSMETICS: &str =
    "https://dados.anvisa.gov.br/dados/CONSULTAS/PRODUTOS/TA_CONSULTA_COSMETICOS.CSV";
pub const CANNABIS: &str =
    "https://dados.anvisa.gov.br/dados/CONSULTAS/PRODUTOS/TA_CONSULTA_PRODUTOS_CANNABIS.CSV";
pub const PORTARIA: &str = "https://anvisalegis.datalegis.net/action/ActionDatalegis.php?acao=abrirTextoAto&cod_menu=8542&cod_modulo=310&link=S&numeroAto=00000344&orgao=SVS%2FMS&seqAto=000&tipo=POR&valorAno=1998";
pub const SIGTAP: &str = "https://github.com/RenatoKR/SIGTAP/raw/refs/heads/main/tabelas/TabelaUnificada_202608_v2608141139.zip";
pub const ANS: &str = "https://www.gov.br/ans/pt-br/arquivos/assuntos/prestadores/padrao-para-troca-de-informacao-de-saude-suplementar-tiss/padrao-tiss-tabelas-relacionadas/padraotiss_mapeamento_tuss_sigtap.zip";

#[derive(Serialize)]
pub struct SourceInfo {
    pub url: String,
    pub sha256: String,
    pub bytes: usize,
    pub download_elapsed_ms: u128,
    pub reused_from_disk: bool,
}
#[derive(Serialize)]
pub struct Manifest {
    pub schema_version: u8,
    pub generated_at: String,
    pub sources: std::collections::BTreeMap<String, SourceInfo>,
    pub counts: std::collections::BTreeMap<String, usize>,
    pub timings_ms: std::collections::BTreeMap<String, u128>,
    pub total_elapsed_ms: u128,
}
#[derive(Serialize, Deserialize)]
pub struct DownloadControl {
    pub schema_version: u8,
    pub files: Vec<DownloadedFile>,
}
#[derive(Serialize, Deserialize)]
pub struct DownloadedFile {
    pub name: String,
    pub filename: String,
    pub downloaded_at: String,
    pub sha1: String,
}

pub fn fetch(
    client: &reqwest::blocking::Client,
    name: &str,
    url: &str,
    reuse_sources: bool,
    output: &Path,
    manifest: &mut Manifest,
    download_control: &mut DownloadControl,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if reuse_sources {
        if let Some(saved) = download_control
            .files
            .iter()
            .rev()
            .find(|entry| entry.name == name)
        {
            let path = output.join(&saved.filename);
            if path.exists() {
                let bytes = std::fs::read(&path)?;
                let actual_sha1 = Sha1::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                if actual_sha1 != saved.sha1 {
                    return Err(format!(
                        "SHA-1 diverge para a fonte preservada {name}: {}",
                        saved.filename
                    )
                    .into());
                }
                let sha256 = Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                eprintln!(
                    "Reutilizando {name}: {} ({} bytes, SHA-1 validado)",
                    saved.filename,
                    bytes.len()
                );
                manifest.sources.insert(
                    name.into(),
                    SourceInfo {
                        url: url.into(),
                        sha256,
                        bytes: bytes.len(),
                        download_elapsed_ms: 0,
                        reused_from_disk: true,
                    },
                );
                return Ok(bytes);
            }
            eprintln!("Fonte preservada ausente ({name}); baixando novamente");
        }
    }
    let started = Instant::now();
    eprintln!("Baixando {name}: {url}");
    let response = client.get(url).send()?.error_for_status()?;
    let bytes = response.bytes()?.to_vec();
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let elapsed = started.elapsed().as_millis();
    let downloaded_at = Utc::now();
    let extension = match name {
        "portaria_344" => "html",
        "sigtap" | "ans_tuss_sigtap" => "zip",
        _ => "csv",
    };
    let filename = format!(
        "sources/{}-{}.{}",
        name,
        downloaded_at.format("%Y%m%dT%H%M%S%.3fZ"),
        extension
    );
    write_atomic(&output.join(&filename), &bytes)?;
    let sha1 = Sha1::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    download_control.files.push(DownloadedFile {
        name: name.into(),
        filename,
        downloaded_at: downloaded_at.to_rfc3339(),
        sha1,
    });
    write_atomic(
        &output.join("download-control.json"),
        &serde_json::to_vec_pretty(download_control)?,
    )?;
    eprintln!(
        "Recebido e preservado {name}: {} bytes em {:.3}s, sha256 {digest}",
        bytes.len(),
        elapsed as f64 / 1000.0
    );
    manifest.sources.insert(
        name.into(),
        SourceInfo {
            url: url.into(),
            sha256: digest,
            bytes: bytes.len(),
            download_elapsed_ms: elapsed,
            reused_from_disk: false,
        },
    );
    Ok(bytes)
}

pub fn record_phase(manifest: &mut Manifest, name: &str, started: Instant) {
    let elapsed = started.elapsed().as_millis();
    manifest.timings_ms.insert(name.to_string(), elapsed);
    eprintln!("Etapa {name}: {:.3}s", elapsed as f64 / 1000.0);
}

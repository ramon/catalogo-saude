use crate::output::write_atomic;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};

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
    let path = fetch_file(
        client,
        name,
        url,
        reuse_sources,
        output,
        manifest,
        download_control,
    )?;
    Ok(fs::read(path)?)
}

pub fn fetch_file(
    client: &reqwest::blocking::Client,
    name: &str,
    url: &str,
    reuse_sources: bool,
    output: &Path,
    manifest: &mut Manifest,
    download_control: &mut DownloadControl,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if reuse_sources {
        if let Some(saved) = download_control
            .files
            .iter()
            .rev()
            .find(|entry| entry.name == name)
        {
            let path = output.join(&saved.filename);
            if path.exists() {
                let digest = hash_source(BufReader::new(File::open(&path)?), None)?;
                if digest.sha1 != saved.sha1 {
                    return Err(format!(
                        "SHA-1 diverge para a fonte preservada {name}: {}",
                        saved.filename
                    )
                    .into());
                }
                eprintln!(
                    "Reutilizando {name}: {} ({} bytes, SHA-1 validado)",
                    saved.filename, digest.bytes
                );
                manifest.sources.insert(
                    name.into(),
                    SourceInfo {
                        url: url.into(),
                        sha256: digest.sha256,
                        bytes: digest.bytes,
                        download_elapsed_ms: 0,
                        reused_from_disk: true,
                    },
                );
                return Ok(path);
            }
            eprintln!("Fonte preservada ausente ({name}); baixando novamente");
        }
    }
    let started = Instant::now();
    eprintln!("Baixando {name}: {url}");
    let response = client.get(url).send()?.error_for_status()?;
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
    let path = output.join(&filename);
    let mut temporary_name = path.as_os_str().to_os_string();
    temporary_name.push(".tmp");
    let temporary_path = PathBuf::from(temporary_name);
    let result = (|| {
        let mut file = File::create(&temporary_path)?;
        let digest = hash_source(response, Some(&mut file))?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, &path)?;
        Ok::<_, Box<dyn std::error::Error>>(digest)
    })();
    let digest = match result {
        Ok(digest) => digest,
        Err(error) => {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
    };
    let elapsed = started.elapsed().as_millis();
    download_control.files.push(DownloadedFile {
        name: name.into(),
        filename,
        downloaded_at: downloaded_at.to_rfc3339(),
        sha1: digest.sha1,
    });
    write_atomic(
        &output.join("download-control.json"),
        &serde_json::to_vec_pretty(download_control)?,
    )?;
    eprintln!(
        "Recebido e preservado {name}: {} bytes em {:.3}s, sha256 {}",
        digest.bytes,
        elapsed as f64 / 1000.0,
        digest.sha256
    );
    manifest.sources.insert(
        name.into(),
        SourceInfo {
            url: url.into(),
            sha256: digest.sha256,
            bytes: digest.bytes,
            download_elapsed_ms: elapsed,
            reused_from_disk: false,
        },
    );
    Ok(path)
}

struct SourceDigest {
    bytes: usize,
    sha1: String,
    sha256: String,
}

fn hash_source(
    mut reader: impl Read,
    mut copy: Option<&mut File>,
) -> Result<SourceDigest, Box<dyn std::error::Error>> {
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        sha1.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
        if let Some(file) = copy.as_mut() {
            file.write_all(&buffer[..count])?;
        }
        bytes += count;
    }
    Ok(SourceDigest {
        bytes,
        sha1: sha1
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        sha256: sha256
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}

pub fn record_phase(manifest: &mut Manifest, name: &str, started: Instant) {
    let elapsed = started.elapsed().as_millis();
    manifest.timings_ms.insert(name.to_string(), elapsed);
    eprintln!("Etapa {name}: {:.3}s", elapsed as f64 / 1000.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn streamed_hashes_and_preserved_bytes_match_whole_source_hashes() {
        let path = std::env::temp_dir().join(format!(
            "catalogo-source-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let bytes = (0..200_000).map(|i| (i % 256) as u8).collect::<Vec<_>>();
        let mut file = File::create(&path).unwrap();
        let streamed = hash_source(Cursor::new(&bytes), Some(&mut file)).unwrap();
        drop(file);
        assert_eq!(streamed.bytes, bytes.len());
        assert_eq!(
            streamed.sha1,
            Sha1::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        assert_eq!(
            streamed.sha256,
            Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reuse_rejects_corrupted_source_before_normalization() {
        let output = std::env::temp_dir().join(format!(
            "catalogo-corrupt-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir_all(output.join("sources")).unwrap();
        fs::write(output.join("sources/example.csv"), b"changed").unwrap();
        let mut control = DownloadControl {
            schema_version: 1,
            files: vec![DownloadedFile {
                name: "example".into(),
                filename: "sources/example.csv".into(),
                downloaded_at: String::new(),
                sha1: "incorrect".into(),
            }],
        };
        let mut manifest = Manifest {
            schema_version: 1,
            generated_at: String::new(),
            sources: Default::default(),
            counts: Default::default(),
            timings_ms: Default::default(),
            total_elapsed_ms: 0,
        };
        let error = fetch_file(
            &reqwest::blocking::Client::new(),
            "example",
            "http://unused.invalid",
            true,
            &output,
            &mut manifest,
            &mut control,
        )
        .unwrap_err();
        assert!(error.to_string().contains("SHA-1 diverge"));
        assert!(manifest.sources.is_empty());
        assert_eq!(
            fs::read(output.join("sources/example.csv")).unwrap(),
            b"changed"
        );
        fs::remove_dir_all(output).unwrap();
    }
}

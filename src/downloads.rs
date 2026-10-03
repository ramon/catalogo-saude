use crate::output::write_atomic;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
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
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub catalog_errors: std::collections::BTreeMap<String, String>,
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
    let mut attempt = 0;
    let digest = loop {
        attempt += 1;
        let result = (|| {
            let response = client.get(url).send()?.error_for_status()?;
            let mut file = File::create(&temporary_path)?;
            let digest = hash_source(response, Some(&mut file))?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary_path, &path)?;
            Ok::<_, Box<dyn std::error::Error>>(digest)
        })();
        match result {
            Ok(digest) => break digest,
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                if attempt >= 4 || !retryable_download_error(error.as_ref()) {
                    return Err(format!(
                        "Falha ao baixar {name} ({url}) após {attempt} tentativa(s): {error:?}"
                    )
                    .into());
                }
                let delay = Duration::from_secs(1 << (attempt - 1));
                eprintln!(
                    "Download de {name} interrompido na tentativa {attempt}/4: {error:?}. Nova tentativa em {}s",
                    delay.as_secs()
                );
                std::thread::sleep(delay);
            }
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

fn retryable_download_error(error: &(dyn std::error::Error + 'static)) -> bool {
    if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        if let Some(status) = error.status() {
            return matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504);
        }
        return error.is_timeout() || error.is_connect() || error.is_request() || error.is_body();
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        if matches!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::TimedOut
        ) {
            return true;
        }
        if let Some(inner) = error.get_ref() {
            return retryable_download_error(inner);
        }
    }
    error.source().is_some_and(retryable_download_error)
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

    fn download_fixture(
        responses: Vec<Option<&'static str>>,
    ) -> (String, std::thread::JoinHandle<usize>) {
        use std::io::BufRead;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/source.csv", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + std::time::Duration::from_secs(12);
            let mut requests = 0;
            for response in responses {
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            std::thread::sleep(std::time::Duration::from_millis(5))
                        }
                        _ => return requests,
                    }
                };
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(&socket);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                requests += 1;
                if let Some(response) = response {
                    socket.write_all(response.as_bytes()).unwrap();
                }
                socket.shutdown(std::net::Shutdown::Both).unwrap();
            }
            requests
        });
        (url, server)
    }

    fn empty_manifest() -> Manifest {
        Manifest {
            schema_version: 1,
            generated_at: String::new(),
            sources: Default::default(),
            counts: Default::default(),
            timings_ms: Default::default(),
            total_elapsed_ms: 0,
            catalog_errors: Default::default(),
        }
    }

    #[test]
    fn interrupted_request_is_retried_without_duplicate_sources() {
        let (url, server) = download_fixture(vec![
            None,
            Some("HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture"),
        ]);
        let output = std::env::temp_dir().join(format!(
            "catalogo-retry-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir_all(output.join("sources")).unwrap();
        let mut control = DownloadControl {
            schema_version: 1,
            files: vec![],
        };
        let mut manifest = empty_manifest();
        let result = fetch_file(
            &reqwest::blocking::Client::builder()
                .no_proxy()
                .build()
                .unwrap(),
            "example",
            &url,
            false,
            &output,
            &mut manifest,
            &mut control,
        );
        assert!(
            result.is_ok(),
            "interrupted connection must be retried: {result:?}"
        );
        let path = result.unwrap();
        assert_eq!(fs::read(path).unwrap(), b"fixture");
        assert_eq!(server.join().unwrap(), 2);
        assert_eq!(control.files.len(), 1);
        assert_eq!(manifest.sources.len(), 1);
        assert_eq!(fs::read_dir(output.join("sources")).unwrap().count(), 1);
        fs::remove_dir_all(output).unwrap();
    }

    fn assert_download_case(
        responses: Vec<Option<&'static str>>,
        expected_requests: usize,
        success: bool,
    ) {
        let (url, server) = download_fixture(responses);
        let output = std::env::temp_dir().join(format!(
            "catalogo-download-case-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir_all(output.join("sources")).unwrap();
        let mut control = DownloadControl {
            schema_version: 1,
            files: vec![],
        };
        let mut manifest = empty_manifest();
        let result = fetch_file(
            &reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            "example",
            &url,
            false,
            &output,
            &mut manifest,
            &mut control,
        );
        assert_eq!(result.is_ok(), success, "{result:?}");
        assert_eq!(server.join().unwrap(), expected_requests);
        if success {
            assert_eq!(fs::read(result.unwrap()).unwrap(), b"fixture");
            assert_eq!(control.files.len(), 1);
            assert_eq!(manifest.sources.len(), 1);
        } else {
            assert!(control.files.is_empty());
            assert!(manifest.sources.is_empty());
            assert!(!output.join("download-control.json").exists());
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("Falha ao baixar example")
            );
        }
        assert_eq!(
            fs::read_dir(output.join("sources")).unwrap().count(),
            usize::from(success)
        );
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn truncated_body_restarts_download_and_discards_partial_file() {
        assert_download_case(
            vec![
                Some("HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfix"),
                Some("HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture"),
            ],
            2,
            true,
        );
    }

    #[test]
    fn temporary_http_failure_is_retried() {
        assert_download_case(
            vec![
                Some(
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                ),
                Some("HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture"),
            ],
            2,
            true,
        );
    }

    #[test]
    fn permanent_http_failure_is_not_retried() {
        assert_download_case(
            vec![Some(
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )],
            1,
            false,
        );
        assert!(!retryable_download_error(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
    }

    #[test]
    fn persistent_failure_exhausts_four_attempts_without_registering_a_source() {
        assert_download_case(
            vec![
                Some(
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                4
            ],
            4,
            false,
        );
    }

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
            catalog_errors: Default::default(),
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

use serde_json::json;
use sha1::{Digest, Sha1};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture(corrupt_medicine_hash: bool) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "catalogo-isolation-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("sources")).unwrap();
    let sources = [
        (
            "medicines_open",
            "NUMERO_PROCESSO;NOME_PRODUTO;SITUACAO_REGISTRO\n1;Example;Ativo\n",
        ),
        (
            "medicines_consultation",
            "NU_PROCESSO;NO_PRODUTO;VALIDADE_SITUACAO\n1;Example;Ativo\n",
        ),
        ("medicines_prices", "NU_REGISTRO;DS_APRESENTACAO\n"),
        ("cosmetics", "INVALID_HEADER\nfixture\n"),
        (
            "cannabis",
            "NU_PROCESSO;NO_PRODUTO;NU_REGISTRO_PRODUTO;SITUACAO_VALIDADE\n2;Example cannabis;123;Válido\n",
        ),
        (
            "portaria_344",
            "<p>LISTA - A1</p><p>LISTA DAS SUBSTANCIAS ENTORPECENTES</p><p>(Sujeitas a Notificacao de Receita A)</p><p>1. Morfina</p>",
        ),
        ("sigtap", "INVALID ZIP"),
        ("ans_tuss_sigtap", "INVALID ZIP"),
    ];
    let mut files = Vec::new();
    for (name, contents) in sources {
        // The inputs use Windows-1252, like the public ANVISA CSV sources.
        let bytes = encoding_rs::WINDOWS_1252.encode(contents).0.into_owned();
        let filename = format!("sources/{name}.raw");
        fs::write(root.join(&filename), &bytes).unwrap();
        let hash = if corrupt_medicine_hash && name == "medicines_open" {
            "invalid".into()
        } else {
            Sha1::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        files.push(json!({"name":name,"filename":filename,"downloaded_at":"2026-10-02T00:00:00Z","sha1":hash}));
    }
    fs::write(
        root.join("download-control.json"),
        serde_json::to_vec(&json!({"schema_version":1,"files":files})).unwrap(),
    )
    .unwrap();
    Fixture(root)
}

fn assert_isolation(corrupt_medicine_hash: bool) {
    let fixture = fixture(corrupt_medicine_hash);
    let output = Command::new(env!("CARGO_BIN_EXE_catalogo-saude"))
        .arg("--output")
        .arg(&fixture.0)
        .arg("--reuse-sources")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let logs = String::from_utf8_lossy(&output.stderr);
    let manifest_path = fixture.0.join("manifest.json");
    assert!(
        manifest_path.exists(),
        "A failed catalog must not abort the run before the manifest: {logs}"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert!(manifest["catalog_errors"]["cosmetics"].is_string());
    assert!(manifest["catalog_errors"]["sigtap"].is_string());
    assert_eq!(manifest["counts"]["cannabis"], 1);
    assert_eq!(manifest["counts"]["portaria_344_lists"], 1);
    assert!(manifest["counts"].get("cosmetics").is_none());
    if corrupt_medicine_hash {
        assert!(manifest["catalog_errors"]["medicines"].is_string());
        assert!(manifest["counts"].get("medicines").is_none());
    } else {
        assert_eq!(manifest["counts"]["medicines"], 1);
    }
    assert!(fixture.0.join("cannabis/ativo/batch-000001.json").is_file());
    assert!(
        fixture
            .0
            .join("portaria-344/lists/batch-000001.json")
            .is_file()
    );
    assert!(logs.contains("[5/5]"));
    assert!(logs.contains("Coleta concluída com falhas"));
    assert_eq!(fs::read_dir(fixture.0.join("sources")).unwrap().count(), 8);
}

#[test]
fn failed_cosmetics_does_not_stop_later_catalogs() {
    assert_isolation(false);
}

#[test]
fn medicine_source_failure_does_not_prevent_independent_portaria_processing() {
    assert_isolation(true);
}

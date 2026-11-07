use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs, path::Path};

const SOURCE_PROJECT: &str = "Precisa-Saude/fhir-brasil";

#[derive(Deserialize)]
struct Catalog {
    source_version: String,
    biomarkers: Vec<Value>,
    sources: Vec<Value>,
}

pub fn collect(
    project_root: &Path,
) -> Result<(Vec<Value>, Vec<Value>), Box<dyn std::error::Error>> {
    let path = project_root.join("references/biomarkers.json");
    let catalog: Catalog = serde_json::from_slice(&fs::read(&path)?)?;
    let biomarkers = catalog
        .biomarkers
        .into_iter()
        .map(|mut row| {
            row["catalog_source"] = json!(SOURCE_PROJECT);
            row["catalog_source_version"] = json!(catalog.source_version);
            row
        })
        .collect();
    let sources = catalog
        .sources
        .into_iter()
        .map(|mut row| {
            row["catalog_source"] = json!(SOURCE_PROJECT);
            row["catalog_source_version"] = json!(catalog.source_version);
            row
        })
        .collect();
    Ok((biomarkers, sources))
}

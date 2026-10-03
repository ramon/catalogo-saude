mod anvisa;
mod cosmetics;
mod downloads;
mod output;
mod portaria;
mod prescription;
mod sigtap;

use anvisa::{normalize_cannabis, normalize_medicines, normalize_portaria};
use chrono::Utc;
use clap::Parser;
use cosmetics::normalize_cosmetics;
use downloads::{
    ANS, CANNABIS, COSMETICS, DownloadControl, MED_CONSULT, MED_OPEN, MED_PRICES, Manifest,
    PORTARIA, SIGTAP, fetch, fetch_file, record_phase,
};
use output::{write_atomic, write_batches, write_batches_by_status};
use sigtap::normalize_sigtap;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(about = "Baixa fontes de catálogos e grava lotes JSON normalizados")]
struct Args {
    #[arg(long, default_value = "./runs/current")]
    output: PathBuf,
    #[arg(long, default_value_t = 1000)]
    batch_size: usize,
    #[arg(
        long,
        help = "Reutiliza fontes brutas listadas em download-control.json quando disponíveis"
    )]
    reuse_sources: bool,
}

fn reference_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let candidates = [
        executable.parent().map(PathBuf::from),
        Some(std::env::current_dir()?),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
    ];
    for root in candidates.into_iter().flatten() {
        if [
            "references/presentation_abbreviations.json",
            "references/anvisa/formas_fisicas.json",
            "references/anvisa/restricao.json",
            "references/anvisa/tarja.json",
        ]
        .iter()
        .all(|file| root.join(file).is_file())
        {
            return Ok(root);
        }
    }
    Err("Referências não encontradas: mantenha references/ junto ao executável ou execute na pasta do projeto".into())
}

fn run_catalog(
    name: &str,
    manifest: &mut Manifest,
    job: impl FnOnce(&mut Manifest) -> Result<(), Box<dyn std::error::Error>>,
) {
    let previous_counts = manifest.counts.clone();
    let started = Instant::now();
    if let Err(error) = job(manifest) {
        manifest.counts = previous_counts;
        let message = error.to_string();
        eprintln!("Falha no catálogo {name}: {message}. Continuando com os demais catálogos");
        manifest.catalog_errors.insert(name.into(), message);
        record_phase(manifest, &format!("failed.{name}"), started);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let run_started = Instant::now();
    let args = Args::parse();
    if args.batch_size == 0 {
        return Err("--batch-size deve ser maior que zero".into());
    }
    eprintln!(
        "Iniciando coleta: saída={}, tamanho dos lotes={}",
        args.output.display(),
        args.batch_size
    );
    fs::create_dir_all(&args.output)?;
    fs::create_dir_all(args.output.join("sources"))?;
    let manifest_path = args.output.join("manifest.json");
    if manifest_path.exists() {
        fs::remove_file(&manifest_path)?;
    }
    let control_path = args.output.join("download-control.json");
    let mut download_control = if control_path.exists() {
        serde_json::from_slice(&fs::read(&control_path)?)?
    } else {
        DownloadControl {
            schema_version: 1,
            files: Vec::new(),
        }
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(30))
        .user_agent(concat!("catalogo-saude/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut manifest = Manifest {
        schema_version: 1,
        generated_at: Utc::now().to_rfc3339(),
        sources: BTreeMap::new(),
        counts: BTreeMap::new(),
        timings_ms: BTreeMap::new(),
        total_elapsed_ms: 0,
        catalog_errors: BTreeMap::new(),
    };
    let mut portaria_rows = None;
    run_catalog("medicines", &mut manifest, |manifest| {
        let project_root = reference_root()?;
        eprintln!(
            "Referências locais: {}",
            project_root.join("references").display()
        );
        eprintln!("[1/5] Medicamentos: obtenção e cruzamento das três fontes");
        let med_open = fetch(
            &client,
            "medicines_open",
            MED_OPEN,
            args.reuse_sources,
            &args.output,
            manifest,
            &mut download_control,
        )?;
        let med_consult = fetch(
            &client,
            "medicines_consultation",
            MED_CONSULT,
            args.reuse_sources,
            &args.output,
            manifest,
            &mut download_control,
        )?;
        let med_prices = fetch(
            &client,
            "medicines_prices",
            MED_PRICES,
            args.reuse_sources,
            &args.output,
            manifest,
            &mut download_control,
        )?;

        eprintln!("Portaria 344: obtenção das listas para classificação dos medicamentos");
        let portaria = fetch(
            &client,
            "portaria_344",
            PORTARIA,
            args.reuse_sources,
            &args.output,
            manifest,
            &mut download_control,
        )?;
        let phase_started = Instant::now();
        portaria_rows = Some(normalize_portaria(&portaria)?);
        drop(portaria);
        record_phase(manifest, "normalize.portaria_344", phase_started);
        let phase_started = Instant::now();
        let medicine_rows = normalize_medicines(
            &med_open,
            &med_consult,
            &med_prices,
            &project_root,
            portaria_rows
                .as_deref()
                .ok_or("Listas da Portaria 344 indisponíveis")?,
        )?;
        drop((med_open, med_consult, med_prices));
        manifest.counts.insert(
            "medicines".into(),
            write_batches_by_status(
                &args.output,
                "medicines",
                medicine_rows,
                args.batch_size,
                "Ativo",
                "Inativo",
            )?,
        );
        record_phase(manifest, "normalize_and_write.medicines", phase_started);

        Ok(())
    });
    run_catalog("cosmetics", &mut manifest, |manifest| {
        eprintln!("[2/5] Cosméticos");
        let cosmetics = fetch_file(
            &client,
            "cosmetics",
            COSMETICS,
            args.reuse_sources,
            &args.output,
            manifest,
            &mut download_control,
        )?;
        let phase_started = Instant::now();
        let cosmetic_count = normalize_cosmetics(&cosmetics, &args.output, args.batch_size)?;
        manifest.counts.insert("cosmetics".into(), cosmetic_count);
        record_phase(manifest, "normalize_and_write.cosmetics", phase_started);
        Ok(())
    });
    run_catalog("cannabis", &mut manifest, |manifest| {
        eprintln!("[3/5] Produtos de cannabis");
        let cannabis = fetch(
            &client,
            "cannabis",
            CANNABIS,
            args.reuse_sources,
            &args.output,
            manifest,
            &mut download_control,
        )?;
        let phase_started = Instant::now();
        let cannabis_rows = normalize_cannabis(&cannabis)?;
        drop(cannabis);
        manifest.counts.insert(
            "cannabis".into(),
            write_batches_by_status(
                &args.output,
                "cannabis",
                cannabis_rows,
                args.batch_size,
                "Válido",
                "Caduco/Cancelado",
            )?,
        );
        record_phase(manifest, "normalize_and_write.cannabis", phase_started);

        Ok(())
    });
    run_catalog("portaria_344", &mut manifest, |manifest| {
        eprintln!("[4/5] Portaria 344: gravação das listas");
        if portaria_rows.is_none() {
            let raw = fetch(
                &client,
                "portaria_344",
                PORTARIA,
                args.reuse_sources,
                &args.output,
                manifest,
                &mut download_control,
            )?;
            let started = Instant::now();
            portaria_rows = Some(normalize_portaria(&raw)?);
            record_phase(manifest, "normalize.portaria_344", started);
        }
        let phase_started = Instant::now();
        manifest.counts.insert(
            "portaria_344_lists".into(),
            write_batches(
                &args.output,
                "portaria-344/lists",
                portaria_rows
                    .take()
                    .ok_or("Listas da Portaria 344 indisponíveis")?,
                args.batch_size,
            )?,
        );
        record_phase(manifest, "normalize_and_write.portaria_344", phase_started);
        Ok(())
    });
    run_catalog("sigtap", &mut manifest, |manifest| {
        eprintln!("[5/5] SIGTAP, TUSS e CID-10");
        let sigtap = fetch(
            &client,
            "sigtap",
            SIGTAP,
            args.reuse_sources,
            &args.output,
            manifest,
            &mut download_control,
        )?;
        let ans = fetch(
            &client,
            "ans_tuss_sigtap",
            ANS,
            args.reuse_sources,
            &args.output,
            manifest,
            &mut download_control,
        )?;
        let phase_started = Instant::now();
        let (procedures, mappings, cids, cid_links) = normalize_sigtap(&sigtap, &ans)?;
        drop((sigtap, ans));
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
        record_phase(
            manifest,
            "normalize_and_write.sigtap_tuss_cid10",
            phase_started,
        );
        Ok(())
    });
    manifest.total_elapsed_ms = run_started.elapsed().as_millis();
    eprintln!(
        "Tempo total da coleta: {:.3}s",
        manifest.total_elapsed_ms as f64 / 1000.0
    );
    write_atomic(
        &args.output.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    if !manifest.catalog_errors.is_empty() {
        return Err(format!(
            "Coleta concluída com falhas nos catálogos: {}. Os demais foram processados; consulte {}",
            manifest.catalog_errors.keys().cloned().collect::<Vec<_>>().join(", "),
            args.output.join("manifest.json").display()
        ).into());
    }
    println!(
        "Concluído: {} registros em {}",
        manifest.counts.values().sum::<usize>(),
        args.output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_catalog_does_not_stop_later_jobs_or_publish_partial_counts() {
        let mut manifest = Manifest {
            schema_version: 1,
            generated_at: String::new(),
            sources: BTreeMap::new(),
            counts: BTreeMap::new(),
            timings_ms: BTreeMap::new(),
            total_elapsed_ms: 0,
            catalog_errors: BTreeMap::new(),
        };
        run_catalog("medicines", &mut manifest, |manifest| {
            let project_root = reference_root()?;
            eprintln!(
                "Referências locais: {}",
                project_root.join("references").display()
            );
            manifest.counts.insert("medicines".into(), 10);
            Ok(())
        });
        run_catalog("cosmetics", &mut manifest, |manifest| {
            manifest.counts.insert("cosmetics".into(), 3);
            Err("interrupted download".into())
        });
        run_catalog("cannabis", &mut manifest, |manifest| {
            manifest.counts.insert("cannabis".into(), 2);
            Ok(())
        });
        assert_eq!(
            manifest.counts,
            BTreeMap::from([("medicines".into(), 10), ("cannabis".into(), 2)])
        );
        assert!(manifest.catalog_errors["cosmetics"].contains("interrupted download"));
        assert!(manifest.timings_ms.contains_key("failed.cosmetics"));
        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(
            json["catalog_errors"]["cosmetics"],
            manifest.catalog_errors["cosmetics"]
        );
        manifest.catalog_errors.clear();
        assert!(
            serde_json::to_value(&manifest)
                .unwrap()
                .get("catalog_errors")
                .is_none()
        );
    }
}

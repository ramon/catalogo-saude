mod anvisa;
mod downloads;
mod output;
mod sigtap;

use anvisa::{normalize_cannabis, normalize_cosmetics, normalize_medicines, normalize_portaria};
use chrono::Utc;
use clap::Parser;
use downloads::{
    ANS, CANNABIS, COSMETICS, DownloadControl, MED_CONSULT, MED_OPEN, MED_PRICES, Manifest,
    PORTARIA, SIGTAP, fetch, record_phase,
};
use output::{write_atomic, write_batches};
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
        .user_agent("catalog-data-loader/0.1")
        .build()?;
    let mut manifest = Manifest {
        schema_version: 1,
        generated_at: Utc::now().to_rfc3339(),
        sources: BTreeMap::new(),
        counts: BTreeMap::new(),
        timings_ms: BTreeMap::new(),
        total_elapsed_ms: 0,
    };
    eprintln!("[1/5] Medicamentos: obtenção e cruzamento das três fontes");
    let med_open = fetch(
        &client,
        "medicines_open",
        MED_OPEN,
        args.reuse_sources,
        &args.output,
        &mut manifest,
        &mut download_control,
    )?;
    let med_consult = fetch(
        &client,
        "medicines_consultation",
        MED_CONSULT,
        args.reuse_sources,
        &args.output,
        &mut manifest,
        &mut download_control,
    )?;
    let med_prices = fetch(
        &client,
        "medicines_prices",
        MED_PRICES,
        args.reuse_sources,
        &args.output,
        &mut manifest,
        &mut download_control,
    )?;
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let phase_started = Instant::now();
    let medicine_rows = normalize_medicines(&med_open, &med_consult, &med_prices, &project_root)?;
    manifest.counts.insert(
        "medicines".into(),
        write_batches(&args.output, "medicines", medicine_rows, args.batch_size)?,
    );
    record_phase(
        &mut manifest,
        "normalize_and_write.medicines",
        phase_started,
    );

    eprintln!("[2/5] Cosméticos");
    let cosmetics = fetch(
        &client,
        "cosmetics",
        COSMETICS,
        args.reuse_sources,
        &args.output,
        &mut manifest,
        &mut download_control,
    )?;
    let phase_started = Instant::now();
    let cosmetic_count = normalize_cosmetics(&cosmetics, &args.output, args.batch_size)?;
    manifest.counts.insert("cosmetics".into(), cosmetic_count);
    record_phase(
        &mut manifest,
        "normalize_and_write.cosmetics",
        phase_started,
    );
    eprintln!("[3/5] Produtos de cannabis");
    let cannabis = fetch(
        &client,
        "cannabis",
        CANNABIS,
        args.reuse_sources,
        &args.output,
        &mut manifest,
        &mut download_control,
    )?;
    let phase_started = Instant::now();
    let cannabis_rows = normalize_cannabis(&cannabis)?;
    manifest.counts.insert(
        "cannabis".into(),
        write_batches(&args.output, "cannabis", cannabis_rows, args.batch_size)?,
    );
    record_phase(&mut manifest, "normalize_and_write.cannabis", phase_started);

    eprintln!("[4/5] Portaria 344");
    let portaria = fetch(
        &client,
        "portaria_344",
        PORTARIA,
        args.reuse_sources,
        &args.output,
        &mut manifest,
        &mut download_control,
    )?;
    let phase_started = Instant::now();
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
    record_phase(
        &mut manifest,
        "normalize_and_write.portaria_344",
        phase_started,
    );
    eprintln!("[5/5] SIGTAP, TUSS e CID-10");
    let sigtap = fetch(
        &client,
        "sigtap",
        SIGTAP,
        args.reuse_sources,
        &args.output,
        &mut manifest,
        &mut download_control,
    )?;
    let ans = fetch(
        &client,
        "ans_tuss_sigtap",
        ANS,
        args.reuse_sources,
        &args.output,
        &mut manifest,
        &mut download_control,
    )?;
    let phase_started = Instant::now();
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
    record_phase(
        &mut manifest,
        "normalize_and_write.sigtap_tuss_cid10",
        phase_started,
    );
    manifest.total_elapsed_ms = run_started.elapsed().as_millis();
    eprintln!(
        "Tempo total da coleta: {:.3}s",
        manifest.total_elapsed_ms as f64 / 1000.0
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

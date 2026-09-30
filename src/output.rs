use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub fn write_batches(
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
    let batch_count = count.div_ceil(batch_size);
    eprintln!(
        "Gravando {count} registros em {batch_count} lotes: {}",
        dir.display()
    );
    for (i, chunk) in rows.chunks(batch_size).enumerate() {
        let file = dir.join(format!("batch-{:06}.json", i + 1));
        write_atomic(&file, &serde_json::to_vec_pretty(chunk)?)?;
        eprintln!(
            "Lote {}/{} gravado: {} registros",
            i + 1,
            batch_count,
            chunk.len()
        );
    }
    Ok(count)
}

pub fn write_batches_iter<I>(
    root: &Path,
    name: &str,
    rows: I,
    batch_size: usize,
) -> Result<usize, Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = Value>,
{
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
    let mut rows = rows.into_iter();
    let mut batch_index = 0usize;
    let mut row_count = 0usize;
    loop {
        let batch = rows.by_ref().take(batch_size).collect::<Vec<_>>();
        if batch.is_empty() {
            break;
        }
        batch_index += 1;
        row_count += batch.len();
        let path = dir.join(format!("batch-{batch_index:06}.json"));
        write_atomic(&path, &serde_json::to_vec_pretty(&batch)?)?;
        if batch_index == 1 || batch_index % 50 == 0 {
            eprintln!("Lote {batch_index} gravado: {row_count} registros acumulados");
        }
    }
    eprintln!(
        "Gravação concluída: {row_count} registros em {batch_index} lotes ({})",
        dir.display()
    );
    Ok(row_count)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut f = fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(tmp, path)?;
    Ok(())
}

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

fn clear_batches(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("batch-") && name.ends_with(".json"))
        {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn write_batches_by_status<I>(
    root: &Path,
    name: &str,
    rows: I,
    batch_size: usize,
    active_status: &str,
    inactive_status: &str,
) -> Result<usize, Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = Value>,
{
    let mut writer =
        StatusBatchWriter::new(root, name, batch_size, active_status, inactive_status)?;
    for row in rows {
        writer.push(row)?;
    }
    writer.finish()
}

pub(crate) struct StatusBatchWriter {
    parent: PathBuf,
    dirs: [PathBuf; 2],
    buffers: [Vec<Value>; 2],
    batch_indices: [usize; 2],
    counts: [usize; 2],
    batch_size: usize,
    statuses: [String; 2],
}

impl StatusBatchWriter {
    pub(crate) fn new(
        root: &Path,
        name: &str,
        batch_size: usize,
        active_status: &str,
        inactive_status: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if batch_size == 0 {
            return Err("--batch-size deve ser maior que zero".into());
        }
        let parent = root.join(name);
        let dirs = [parent.join("ativo"), parent.join("inativos")];
        for dir in &dirs {
            clear_batches(dir)?;
        }
        Ok(Self {
            parent,
            dirs,
            buffers: [Vec::new(), Vec::new()],
            batch_indices: [0; 2],
            counts: [0; 2],
            batch_size,
            statuses: [active_status.into(), inactive_status.into()],
        })
    }

    pub(crate) fn push(&mut self, row: Value) -> Result<(), Box<dyn std::error::Error>> {
        let status = row["regulatory_status"].as_str().unwrap_or_default();
        let group = self
            .statuses
            .iter()
            .position(|candidate| candidate == status)
            .ok_or_else(|| {
                format!(
                    "situação regulatória desconhecida em {}: {status:?}",
                    self.parent.display()
                )
            })?;
        self.counts[group] += 1;
        self.buffers[group].push(row);
        if self.buffers[group].len() == self.batch_size {
            flush_batch(
                &self.dirs[group],
                &mut self.buffers[group],
                &mut self.batch_indices[group],
            )?;
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<usize, Box<dyn std::error::Error>> {
        for group in 0..2 {
            flush_batch(
                &self.dirs[group],
                &mut self.buffers[group],
                &mut self.batch_indices[group],
            )?;
            eprintln!(
                "Gravação concluída: {} registros em {} lotes ({})",
                self.counts[group],
                self.batch_indices[group],
                self.dirs[group].display()
            );
        }
        // Remove the old flat layout only after both partitions have been written.
        clear_batches(&self.parent)?;
        Ok(self.counts.iter().sum())
    }
}

fn flush_batch(
    dir: &Path,
    batch: &mut Vec<Value>,
    index: &mut usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if batch.is_empty() {
        return Ok(());
    }
    *index += 1;
    let path = dir.join(format!("batch-{:06}.json", *index));
    write_atomic(&path, &serde_json::to_vec_pretty(batch)?)?;
    batch.clear();
    if *index == 1 || *index % 50 == 0 {
        eprintln!("Lote {} gravado em {}", *index, dir.display());
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "catalogo-output-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn read_batch(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn status_batches_keep_rows_and_number_partitions_independently() {
        let root = TestDirectory::new();
        let parent = root.0.join("medicines");
        fs::create_dir_all(parent.join("ativo")).unwrap();
        fs::write(parent.join("batch-000099.json"), b"[]").unwrap();
        fs::write(parent.join("ativo/batch-000099.json"), b"[]").unwrap();
        fs::write(parent.join("notes.json"), b"{}").unwrap();
        let rows = vec![
            json!({"id":1, "regulatory_status":"Ativo", "portaria_344_lists":["A2"]}),
            json!({"id":2, "regulatory_status":"Inativo"}),
            json!({"id":3, "regulatory_status":"Ativo"}),
            json!({"id":4, "regulatory_status":"Inativo"}),
            json!({"id":5, "regulatory_status":"Ativo"}),
        ];
        let count =
            write_batches_by_status(&root.0, "medicines", rows.clone(), 2, "Ativo", "Inativo")
                .unwrap();
        assert_eq!(count, 5);
        assert_eq!(
            read_batch(&parent.join("ativo/batch-000001.json")),
            json!([rows[0], rows[2]])
        );
        assert_eq!(
            read_batch(&parent.join("ativo/batch-000002.json")),
            json!([rows[4]])
        );
        assert_eq!(
            read_batch(&parent.join("inativos/batch-000001.json")),
            json!([rows[1], rows[3]])
        );
        assert!(!parent.join("batch-000099.json").exists());
        assert!(!parent.join("ativo/batch-000099.json").exists());
        assert!(parent.join("notes.json").exists());

        // A second run must also clear a partition that is now empty.
        write_batches_by_status(
            &root.0,
            "medicines",
            vec![rows[1].clone()],
            2,
            "Ativo",
            "Inativo",
        )
        .unwrap();
        assert_eq!(fs::read_dir(parent.join("ativo")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(parent.join("inativos")).unwrap().count(), 1);
        assert_eq!(
            read_batch(&parent.join("inativos/batch-000001.json")),
            json!([rows[1]])
        );
    }

    #[test]
    fn source_status_pairs_route_cannabis_and_cosmetics() {
        let root = TestDirectory::new();
        for (catalog, active, inactive) in [
            ("cannabis", "Válido", "Caduco/Cancelado"),
            ("cosmetics", "S", "N"),
        ] {
            let rows = vec![
                json!({"regulatory_status":inactive}),
                json!({"regulatory_status":active}),
            ];
            write_batches_by_status(&root.0, catalog, rows.clone(), 1, active, inactive).unwrap();
            assert_eq!(
                read_batch(&root.0.join(catalog).join("ativo/batch-000001.json")),
                json!([rows[1]])
            );
            assert_eq!(
                read_batch(&root.0.join(catalog).join("inativos/batch-000001.json")),
                json!([rows[0]])
            );
        }
    }

    #[test]
    fn unknown_status_does_not_silently_become_inactive() {
        let root = TestDirectory::new();
        let parent = root.0.join("cannabis");
        fs::create_dir_all(&parent).unwrap();
        fs::write(parent.join("batch-000001.json"), b"[]").unwrap();
        let error = write_batches_by_status(
            &root.0,
            "cannabis",
            vec![json!({"regulatory_status":"Unknown"})],
            1,
            "Válido",
            "Caduco/Cancelado",
        )
        .unwrap_err();
        assert!(error.to_string().contains("Unknown"));
        assert!(parent.join("batch-000001.json").exists());
        assert_eq!(fs::read_dir(parent.join("inativos")).unwrap().count(), 0);
    }
}

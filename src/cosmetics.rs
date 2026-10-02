use crate::output::StatusBatchWriter;
use encoding_rs::WINDOWS_1252;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter},
    path::{Path, PathBuf},
};

type Error = Box<dyn std::error::Error>;
const PARTITION_COUNT: usize = 32;

pub fn normalize_cosmetics(path: &Path, output: &Path, batch_size: usize) -> Result<usize, Error> {
    normalize_cosmetics_reader(BufReader::new(File::open(path)?), output, batch_size)
}

fn normalize_cosmetics_reader(
    mut source: impl BufRead,
    output: &Path,
    batch_size: usize,
) -> Result<usize, Error> {
    let mut line_bytes = Vec::new();
    if source.read_until(b'\n', &mut line_bytes)? == 0 {
        return Err("CSV de cosméticos sem cabeçalho".into());
    }
    let header_text = WINDOWS_1252.decode(&line_bytes).0;
    let headers = parse_delimited_record(header_text.lines().next().unwrap_or_default())?;
    let columns = CosmeticColumns::new(&headers)?;
    let temporary = TemporaryPartitions::new(output)?;
    let paths = (0..PARTITION_COUNT)
        .map(|i| temporary.0.join(format!("part-{i:02}.csv")))
        .collect::<Vec<_>>();
    let mut partitions = paths
        .iter()
        .map(|path| {
            Ok(csv::WriterBuilder::new()
                .delimiter(b';')
                .has_headers(false)
                .from_writer(BufWriter::new(File::create(path)?)))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    eprintln!(
        "Cosméticos: lendo a fonte e distribuindo processos em {PARTITION_COUNT} partições temporárias"
    );
    let mut row_count = 0;
    loop {
        line_bytes.clear();
        if source.read_until(b'\n', &mut line_bytes)? == 0 {
            break;
        }
        row_count += 1;
        let line_number = row_count + 1;
        let text = WINDOWS_1252.decode(&line_bytes).0;
        let line = text.lines().next().unwrap_or_default();
        let mut record = parse_delimited_record(line).unwrap_or_default();
        if record.len() != headers.len() || !matches!(record.get(columns.status), Some("S" | "N")) {
            record = parse_delimited_record(&line.replacen("\"\";", "\"\"\";", 1))?;
        }
        if record.len() != headers.len() {
            return Err(format!(
                "linha {line_number} possui quantidade inválida de colunas em cosméticos"
            )
            .into());
        }
        let input = columns.input(&record, line_number)?;
        let partition = partition_for(input.id);
        partitions[partition].write_record([
            input.id,
            input.registration_number,
            input.name,
            input.manufacturer,
            if input.is_active { "S" } else { "N" },
        ])?;
        if row_count % 100_000 == 0 {
            eprintln!("Cosméticos: processadas {row_count} linhas");
        }
    }
    for partition in &mut partitions {
        partition.flush()?;
    }
    drop(partitions);
    drop(source);
    let mut writer = StatusBatchWriter::new(output, "cosmetics", batch_size, "S", "N")?;
    for (index, path) in paths.iter().enumerate() {
        let products = read_partition(path)?;
        eprintln!(
            "Cosméticos: partição {}/{} contém {} produtos deduplicados",
            index + 1,
            PARTITION_COUNT,
            products.records.len()
        );
        for row in products.into_rows() {
            writer.push(row)?;
        }
        fs::remove_file(path)?;
    }
    let count = writer.finish()?;
    eprintln!("Cosméticos: {row_count} linhas lidas, {count} produtos após deduplicação");
    Ok(count)
}

fn partition_for(id: &str) -> usize {
    usize::from(Sha256::digest(id.as_bytes())[0]) % PARTITION_COUNT
}

fn read_partition(path: &Path) -> Result<CosmeticProducts, Error> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .has_headers(false)
        .from_reader(BufReader::new(File::open(path)?));
    let mut products = CosmeticProducts::default();
    let mut record = csv::StringRecord::new();
    while reader.read_record(&mut record)? {
        if record.len() != 5 {
            return Err("partição de cosméticos com colunas inválidas".into());
        }
        products.insert(CosmeticInput {
            id: &record[0],
            registration_number: &record[1],
            name: &record[2],
            manufacturer: &record[3],
            is_active: match record.get(4) {
                Some("S") => true,
                Some("N") => false,
                _ => return Err("situação inválida em partição de cosméticos".into()),
            },
        })?;
    }
    Ok(products)
}

struct CosmeticColumns {
    id: usize,
    name: usize,
    registration: usize,
    manufacturer: usize,
    status: usize,
}

impl CosmeticColumns {
    fn new(headers: &csv::StringRecord) -> Result<Self, Error> {
        let column = |name: &str| {
            headers
                .iter()
                .enumerate()
                .filter_map(|(index, header)| (header == name).then_some(index))
                .last()
                .ok_or_else(|| format!("CSV de cosméticos sem coluna {name}"))
        };
        Ok(Self {
            id: column("NU_PROCESSO")?,
            name: column("NO_PRODUTO")?,
            registration: column("NU_REGISTRO")?,
            manufacturer: column("NO_RAZAO_SOCIAL_EMPRESA")?,
            status: column("ST_SITUACAO_PRODUTO")?,
        })
    }

    fn input<'a>(
        &self,
        record: &'a csv::StringRecord,
        line: usize,
    ) -> Result<CosmeticInput<'a>, Error> {
        let field = |index| record.get(index).unwrap_or_default().trim();
        let id = field(self.id);
        let name = field(self.name);
        if id.is_empty() || name.is_empty() {
            return Err(format!("cosmético sem processo/nome na linha {line}").into());
        }
        let status = field(self.status);
        if status != "S" && status != "N" {
            return Err(format!("situação cosmético inválida na linha {line}").into());
        }
        let manufacturer = field(self.manufacturer);
        Ok(CosmeticInput {
            id,
            name,
            registration_number: field(self.registration),
            manufacturer: if manufacturer == "-" {
                ""
            } else {
                manufacturer
            },
            is_active: status == "S",
        })
    }
}

struct CosmeticInput<'a> {
    id: &'a str,
    registration_number: &'a str,
    name: &'a str,
    manufacturer: &'a str,
    is_active: bool,
}

struct CosmeticRecord {
    registration_number: Box<str>,
    name: Box<str>,
    manufacturer_id: usize,
    is_active: bool,
}

#[derive(Default)]
struct CosmeticProducts {
    records: HashMap<Box<str>, CosmeticRecord>,
    manufacturer_ids: HashMap<Box<str>, usize>,
    manufacturers: Vec<Box<str>>,
}

impl CosmeticProducts {
    fn insert(&mut self, input: CosmeticInput<'_>) -> Result<(), Error> {
        if let Some(previous) = self.records.get_mut(input.id) {
            if previous.registration_number.as_ref() != input.registration_number
                || previous.name.as_ref() != input.name
                || self.manufacturers[previous.manufacturer_id].as_ref() != input.manufacturer
            {
                return Err(format!("NU_PROCESSO {} conflitante", input.id).into());
            }
            previous.is_active &= input.is_active;
        } else {
            let manufacturer_id = if let Some(id) = self.manufacturer_ids.get(input.manufacturer) {
                *id
            } else {
                let id = self.manufacturers.len();
                self.manufacturers.push(input.manufacturer.into());
                self.manufacturer_ids.insert(input.manufacturer.into(), id);
                id
            };
            self.records.insert(
                input.id.into(),
                CosmeticRecord {
                    registration_number: input.registration_number.into(),
                    name: input.name.into(),
                    manufacturer_id,
                    is_active: input.is_active,
                },
            );
        }
        Ok(())
    }

    fn into_rows(self) -> impl Iterator<Item = Value> {
        let Self {
            records,
            manufacturer_ids,
            manufacturers,
        } = self;
        drop(manufacturer_ids);
        records.into_iter().map(move |(id, row)| {
            json!({"source_identifier":id, "registration_number":row.registration_number, "name":row.name,
                "manufacturer":manufacturers[row.manufacturer_id], "regulatory_status":if row.is_active { "S" } else { "N" }})
        })
    }
}

fn parse_delimited_record(line: &str) -> Result<csv::StringRecord, csv::Error> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .has_headers(false)
        .flexible(true)
        .from_reader(line.as_bytes());
    let mut record = csv::StringRecord::new();
    reader.read_record(&mut record)?;
    Ok(record)
}

struct TemporaryPartitions(PathBuf);

impl TemporaryPartitions {
    fn new(output: &Path) -> Result<Self, Error> {
        fs::create_dir_all(output)?;
        let path = output.join(format!(
            ".cosmetics-partitions-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TemporaryPartitions {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partitions_merge_distant_duplicates_and_keep_batches_contiguous() {
        let output = std::env::temp_dir().join(format!(
            "catalogo-partitions-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let mut text =
            "NU_PROCESSO;NO_PRODUTO;NO_RAZAO_SOCIAL_EMPRESA;ST_SITUACAO_PRODUTO;NU_REGISTRO\n"
                .to_string();
        for i in 0..50 {
            text.push_str(&format!("{i};Product {i};Shared manufacturer;S;{i}\n"));
        }
        for i in (0..50).rev() {
            text.push_str(&format!(
                "{i};Product {i};Shared manufacturer;{};{i}\n",
                if i % 2 == 0 { "N" } else { "S" }
            ));
        }
        assert_eq!(
            normalize_cosmetics_reader(std::io::Cursor::new(text.as_bytes()), &output, 7).unwrap(),
            50
        );
        for (folder, status) in [("ativo", "S"), ("inativos", "N")] {
            let mut count = 0;
            for index in 1..=4 {
                let rows: Vec<Value> = serde_json::from_slice(
                    &fs::read(
                        output
                            .join("cosmetics")
                            .join(folder)
                            .join(format!("batch-{index:06}.json")),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(rows.len(), if index == 4 { 4 } else { 7 });
                for row in rows {
                    assert_eq!(row["regulatory_status"], status);
                    let id: usize = row["source_identifier"].as_str().unwrap().parse().unwrap();
                    assert_eq!(id % 2 == 0, status == "N");
                    assert_eq!(row["name"], format!("Product {id}"));
                    count += 1;
                }
            }
            assert_eq!(count, 25);
        }
        assert!(!fs::read_dir(&output).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".cosmetics-partitions")
        }));
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn streaming_cosmetics_preserves_encoding_quotes_and_inactive_precedence() {
        let output = std::env::temp_dir().join(format!(
            "catalogo-streaming-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir_all(&output).unwrap();
        let text = "NU_PROCESSO;NO_PRODUTO;NO_RAZAO_SOCIAL_EMPRESA;ST_SITUACAO_PRODUTO;NU_REGISTRO\r\n1;\"Loção “Terra Mãe\"\";Acme;N;\r\n1;\"Loção “Terra Mãe\"\";Acme;S;\r\n2;Outro;Acme;S;123\r\n";
        let bytes = WINDOWS_1252.encode(text).0.into_owned();
        let reader = BufReader::with_capacity(1, std::io::Cursor::new(bytes));
        assert_eq!(normalize_cosmetics_reader(reader, &output, 1).unwrap(), 2);
        let inactive: Value = serde_json::from_slice(
            &fs::read(output.join("cosmetics/inativos/batch-000001.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inactive[0],
            json!({"source_identifier":"1", "name":"Loção “Terra Mãe\"", "manufacturer":"Acme", "registration_number":"", "regulatory_status":"N"})
        );
        let active: Value = serde_json::from_slice(
            &fs::read(output.join("cosmetics/ativo/batch-000001.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(active[0]["source_identifier"], "2");
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn streaming_cosmetics_rejects_conflicting_duplicate_products() {
        let bytes = b"NU_PROCESSO;NO_PRODUTO;NO_RAZAO_SOCIAL_EMPRESA;ST_SITUACAO_PRODUTO;NU_REGISTRO\n1;First;Acme;S;123\n1;Different;Acme;N;123\n";
        let output = std::env::temp_dir().join(format!(
            "catalogo-conflict-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let error =
            normalize_cosmetics_reader(std::io::Cursor::new(bytes), &output, 1).unwrap_err();
        assert!(error.to_string().contains("NU_PROCESSO 1 conflitante"));
        assert!(!fs::read_dir(&output).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".cosmetics-partitions")
        }));
        fs::remove_dir_all(output).unwrap();
    }
}

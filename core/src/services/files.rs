use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use csv::Writer;
use serde::Serialize;

use super::{
    parsers::ImportFileFormat,
    shared::constants::{IN_DIR, OUT_DIR},
};


/// Publish complete exports with a same-directory rename, so concurrent readers
/// retain the previous valid result until the replacement is fully written.
fn atomic_export(
    path: &Path,
    write: impl FnOnce(&mut File) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    static NEXT_EXPORT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| anyhow::anyhow!("Export requires a filename"))?;
    let temporary = path.with_file_name(format!(
        ".{}.{}.{}.tmp", name.to_string_lossy(), std::process::id(),
        NEXT_EXPORT.fetch_add(1, Ordering::Relaxed),
    ));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    let result = (|| {
        write(&mut file)?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn create_dir_if_nonexistent(directory_path: &str) {
    let path = Path::new(directory_path);
    if !path.exists() {
        fs::create_dir_all(path).unwrap();
        println!("Folder created at: {:?}", path);
    } else {
        println!("Folder already exists at: {:?}.", path);
    }
}

pub fn create_necessary_directories() {
    create_dir_if_nonexistent(OUT_DIR);
    create_dir_if_nonexistent(IN_DIR);
}

pub fn export_csv<T>(rows: &Vec<T>, file_name: &str) -> anyhow::Result<()>
where
    T: Serialize,
{
    let path = format!("{}/{}.csv", OUT_DIR, file_name);
    atomic_export(Path::new(&path), |file| {
        let mut writer = Writer::from_writer(file);
        for row in rows {
            writer.serialize(row)?;
        }
        writer.flush()?;
        Ok(())
    })
}

pub fn export_json<T>(data: T, file_name: &str) -> anyhow::Result<()>
where
    T: Serialize,
{
    let path = format!("{}/{}.json", OUT_DIR, file_name);
    atomic_export(Path::new(&path), |file| {
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, &data)?;
        writer.flush()?;
        Ok(())
    })
}

pub fn detect_file_format(file: &[u8], file_path: &Path) -> ImportFileFormat {
    if file.is_empty() {
        return ImportFileFormat::Unsupported;
    }

    if file_path.extension() == Some(std::ffi::OsStr::new("pdf")) && file.starts_with(b"%PDF-") {
        return ImportFileFormat::Pdf;
    }

    if file_path.extension() == Some(std::ffi::OsStr::new("csv")) {
        return ImportFileFormat::Csv;
    }

    ImportFileFormat::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;


    #[test]
    fn failed_export_preserves_previous_result_and_cleans_partial_output() {
        let directory = std::env::temp_dir().join(format!(
            "doughbox_export_{}_{}", std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("performance.json");
        fs::write(&path, br#"{"complete":1}"#).unwrap();
        let failure = atomic_export(&path, |file| {
            file.write_all(br#"{"partial":"#)?;
            anyhow::bail!("serialization failed")
        });
        assert!(failure.is_err());
        assert_eq!(fs::read(&path).unwrap(), br#"{"complete":1}"#);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        atomic_export(&path, |file| {
            file.write_all(br#"{"complete":2}"#)?;
            Ok(())
        }).unwrap();
        assert_eq!(fs::read(&path).unwrap(), br#"{"complete":2}"#);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn detect_pdf_csv_and_unsupported() {
        assert_eq!(
            detect_file_format(b"%PDF-1.4 rest", Path::new("statement.pdf")),
            ImportFileFormat::Pdf
        );
        assert_eq!(
            detect_file_format(b"Action,ISIN", Path::new("export.csv")),
            ImportFileFormat::Csv
        );
        assert_eq!(
            detect_file_format(b"", Path::new("empty.csv")),
            ImportFileFormat::Unsupported
        );
        assert_eq!(
            detect_file_format(b"not a pdf", Path::new("statement.pdf")),
            ImportFileFormat::Unsupported
        );
        assert_eq!(
            detect_file_format(b"hello", Path::new("notes.txt")),
            ImportFileFormat::Unsupported
        );
    }
}

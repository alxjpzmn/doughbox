use std::{
    fs::{self, File},
    path::Path,
};

use csv::Writer;
use serde::Serialize;
use serde_json::json;

use super::{
    parsers::ImportFileFormat,
    shared::constants::{IN_DIR, OUT_DIR},
};

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
    let file = File::create(format!("{}/{}.csv", OUT_DIR, file_name))?;
    let mut wtr = Writer::from_writer(file);

    for row in rows {
        wtr.serialize(row)?;
    }

    wtr.flush()?;

    Ok(())
}

pub fn export_json<T>(data: T, file_name: &str) -> anyhow::Result<()>
where
    T: Serialize,
{
    let json_data = json!(&data).to_string();

    std::fs::write(format!("{}/{}.json", OUT_DIR, file_name), json_data)?;

    Ok(())
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

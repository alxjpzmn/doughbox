pub enum FileFormat {
    Pdf,
    Csv,
    Folder,
    Unsupported,
}
pub fn detect_file_format(file_path: &str) -> FileFormat {
    if file_path.ends_with(".pdf") {
        FileFormat::Pdf
    } else if file_path.ends_with(".csv") {
        FileFormat::Csv
    } else if file_path.contains(".") {
        FileFormat::Folder
    } else {
        FileFormat::Unsupported
    }
}

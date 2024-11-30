use std::fs;

use walkdir::WalkDir;

use crate::services::parsers::parse_file_for_import;

pub async fn import(directory_path: &str) -> anyhow::Result<()> {
    for entry in WalkDir::new(directory_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file_path = entry.path();

        match fs::read(file_path) {
            Ok(buffer) => {
                if let Err(e) = parse_file_for_import(&buffer).await {
                    eprintln!("Failed to process {}: {:?}", file_path.display(), e);
                    continue;
                }
            }
            Err(e) => {
                eprintln!("Failed to read {}: {:?}", file_path.display(), e);
                continue;
            }
        }
    }
    Ok(())
}

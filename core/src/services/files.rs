use std::{fs, path::Path};

use super::constants::{IN_DIR, OUT_DIR};

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

use static_files::resource_dir;
use std::fs;
use std::path::Path;

fn main() -> std::io::Result<()> {
    if !Path::new("./dist").exists() {
        fs::create_dir_all("./dist")?;
    }
    resource_dir("./dist").build()
}

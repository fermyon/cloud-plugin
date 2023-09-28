use std::path::PathBuf;

pub fn bin_path() -> anyhow::Result<PathBuf> {
    let bin_path = std::env::var("SPIN_BIN_PATH")?;
    Ok(PathBuf::from(bin_path))
}

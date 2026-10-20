//! Paths and env files that every command shares.

use std::path::PathBuf;

/// ${DATA}, default ~/homelab-data: the encrypted vault with app data and secrets.
pub fn data_dir() -> PathBuf {
    std::env::var_os("DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("homelab-data"))
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is not set"))
}

/// KEY=value lines of an env file; a missing file gives no keys, as in the Python scripts.
pub fn load_env(path: &std::path::Path) -> std::collections::HashMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

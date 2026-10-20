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

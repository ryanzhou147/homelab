//! Sends a NOTIFY_CMD alert when the lab's vault is still locked after a boot: the lab stays off
//! until someone runs vault-unlock.sh with the master password. A systemd user timer runs it a
//! few minutes after each boot, then every 6 hours; it stays quiet while the vault is open.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use anyhow::Result;

use crate::common::{data_dir, notify, repo_dir};

/// encrypt-data.sh keeps the vault here; the folder is root-only, but its presence is visible.
const VAULT_DIR: &str = "/var/lib/homelab-vault";

#[derive(clap::Args)]
pub struct Args {}

/// A mounted folder sits on another device than its parent.
fn is_mountpoint(path: &Path) -> bool {
    let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
    match (dev(path), path.parent().and_then(dev)) {
        (Some(own), Some(parent)) => own != parent,
        _ => false,
    }
}

/// The lab has a vault, and it is not open.
pub fn is_locked() -> bool {
    Path::new(VAULT_DIR).is_dir() && !is_mountpoint(&data_dir())
}

pub fn run(_args: Args) -> Result<()> {
    if !is_locked() {
        return Ok(());
    }
    let unlock = repo_dir().join("scripts/vault-unlock.sh");
    notify(&format!(
        "The home lab restarted and is locked. Unlock it over SSH: sudo {}",
        unlock.canonicalize().unwrap_or(unlock).display()
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_a_mount_from_a_plain_folder() {
        assert!(is_mountpoint(Path::new("/proc")));
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_mountpoint(dir.path()));
    }
}

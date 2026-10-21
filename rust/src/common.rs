//! Paths and env files that every command shares.

use std::path::PathBuf;

/// A lab setting from the environment, else from the repo's .env.
pub fn setting(key: &str) -> Option<String> {
    let non_empty = |v: String| (!v.is_empty()).then_some(v);
    std::env::var(key).ok().and_then(non_empty).or_else(|| {
        load_env(&repo_dir().join(".env"))
            .remove(key)
            .and_then(non_empty)
    })
}

/// ${DATA}, default ~/homelab-data: the encrypted vault with app data and secrets.
pub fn data_dir() -> PathBuf {
    setting("DATA").map_or_else(|| home().join("homelab-data"), PathBuf::from)
}

/// ${MEDIA}, default ~/homelab-media: music, video and downloads.
pub fn media_dir() -> PathBuf {
    setting("MEDIA").map_or_else(|| home().join("homelab-media"), PathBuf::from)
}

/// The admin's account name in the lab login, Jellyfin and Navidrome (LAB_USER).
pub fn lab_user() -> String {
    setting("LAB_USER").unwrap_or_else(|| "admin".into())
}

/// NAVIDROME_URL, else Navidrome's address under the lab's name.
pub fn navidrome_url() -> Option<String> {
    setting("NAVIDROME_URL").or_else(|| setting("HOST").map(|h| format!("https://{h}/music")))
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

/// Runs NOTIFY_CMD with the message as its one argument, if .env sets it (e.g. notify-send).
pub fn notify(message: &str) -> std::io::Result<()> {
    let Some(cmd) = setting("NOTIFY_CMD") else {
        return Ok(());
    };
    let status = std::process::Command::new(cmd).arg(message).status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "NOTIFY_CMD exited with {status}"
        )));
    }
    Ok(())
}

/// The repo checkout this binary was built from: compose.yml and .env live there.
pub fn repo_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

/// Replaces a file's content through a temp file in the same folder and a rename, so a kill or a
/// full disk mid-write leaves the old file whole. Keeps the old file's permissions.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    // Through a symlink to its target, as mutagen wrote.
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.homelab-tmp"));
    let written = std::fs::write(&tmp, bytes).and_then(|()| match std::fs::metadata(path) {
        Ok(meta) => std::fs::set_permissions(&tmp, meta.permissions()),
        Err(_) => Ok(()),
    });
    if let Err(err) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp); // a full disk must not keep a half copy
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn write_atomic_replaces_bytes_keeps_mode_and_follows_links() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("song.mp3");
        std::fs::write(&file, b"old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.path().join("link.mp3");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        super::write_atomic(&link, b"new").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}

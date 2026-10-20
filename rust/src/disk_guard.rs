//! Stops the music downloads when the media disk gets low, because slskd has no size limit of its
//! own. A systemd user timer runs it every 10 minutes. Start them again with
//!   docker compose start slskd soularr

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, ensure};

use crate::common::{home, load_env, repo_dir};

const MIN_FREE_GB: u64 = 20;
const GIB: u64 = 1 << 30;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(_args: Args) -> Result<()> {
    let repo = repo_dir();
    let env = load_env(&repo.join(".env"));
    let media = env.get("MEDIA").context("MEDIA is not set in .env")?;
    let Some(free_gb) = low_gb(df_avail(Path::new(media), "-B1")?) else {
        return Ok(());
    };
    let compose = |args: &[&str]| {
        let mut c = Command::new("docker");
        c.arg("compose").args(args).current_dir(&repo);
        c
    };
    // A failed docker ps counts as nothing running, as in the shell script.
    let running = compose(&["ps", "-q", "--status", "running", "slskd", "soularr"])
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    if running.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }

    ensure!(
        compose(&["stop", "soularr", "slskd"]).status()?.success(),
        "docker compose stop failed"
    );
    let message = format!("Music downloads stopped: only {free_gb} GB free on the media disk.");
    println!("{message}");
    let notify = home().join(".claude/harness/notify.py");
    if notify
        .metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    {
        ensure!(
            Command::new(&notify).arg(&message).status()?.success(),
            "notify failed"
        );
    }
    Ok(())
}

/// Whole free GiB when the disk is below the limit, else None.
fn low_gb(free_bytes: u64) -> Option<u64> {
    (free_bytes < MIN_FREE_GB * GIB).then_some(free_bytes / GIB)
}

/// Free space on the disk of `path` from `df --output=avail <block>`, as the shell scripts read it.
/// df rounds up to the block size.
pub(crate) fn df_avail(path: &Path, block: &str) -> Result<u64> {
    let out = Command::new("df")
        .args(["--output=avail", block])
        .arg(path)
        .output()?;
    ensure!(out.status.success(), "df failed on {}", path.display());
    Ok(parse_df(&String::from_utf8_lossy(&out.stdout)))
}

/// `tail -1 | tr -dc 0-9`; no digits gives 0, as `${FREE_GB:-0}` does.
fn parse_df(text: &str) -> u64 {
    let last = text.lines().last().unwrap_or("");
    last.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_only_below_20_gib() {
        assert_eq!(low_gb(20 * GIB), None);
        assert_eq!(low_gb(20 * GIB - 1), Some(19));
        assert_eq!(low_gb(5 * GIB + 7), Some(5));
        assert_eq!(low_gb(0), Some(0));
    }

    #[test]
    fn reads_the_last_df_line() {
        assert_eq!(parse_df("    Avail\n  123G\n"), 123);
        assert_eq!(parse_df("Avail\n21474836480\n"), 20 * GIB);
        assert_eq!(parse_df(""), 0);
    }
}

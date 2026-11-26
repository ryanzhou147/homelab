//! Opens a terminal that unlocks the lab when you come back to the desktop and the vault is locked.
//! Auto-login starts the desktop at boot while nobody is there, and sudo stops waiting for a
//! password after 5 minutes, so the window opens on your first input, not at the session start.

use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::common::repo_dir;
use crate::locked_alert::is_locked;

/// No input for this long counts as away: a window you closed opens again when you come back.
const AWAY_MS: u64 = 10 * 60 * 1000;
const POLL: Duration = Duration::from_secs(2);

#[derive(clap::Args)]
pub struct Args {}

/// Milliseconds since the last keyboard or mouse input, from GNOME's idle monitor.
fn idle_ms() -> Result<u64> {
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Mutter.IdleMonitor",
            "--object-path",
            "/org/gnome/Mutter/IdleMonitor/Core",
            "--method",
            "org.gnome.Mutter.IdleMonitor.GetIdletime",
        ])
        .stderr(std::process::Stdio::inherit())
        .output()
        .context("run gdbus")?;
    parse_idle(&String::from_utf8_lossy(&out.stdout)).context("read the idle time from GNOME")
}

/// gdbus prints "(uint64 1234,)".
fn parse_idle(text: &str) -> Option<u64> {
    text.trim()
        .strip_prefix("(uint64 ")?
        .strip_suffix(",)")?
        .parse()
        .ok()
}

/// (away after this poll, open the window now). The idle time only drops when there was input,
/// so a desktop that auto-login started with nobody there never opens the window.
fn step(away: bool, prev_idle: u64, idle: u64) -> (bool, bool) {
    if idle >= AWAY_MS {
        return (true, false);
    }
    let input = idle < prev_idle;
    (away && !input, away && input)
}

fn prompt() -> Result<()> {
    let unlock = repo_dir().join("scripts/vault-unlock.sh");
    // Retries in the same window; closing the window skips the unlock until you come back.
    let script = r#"echo "The home lab is locked."
until sudo "$1"; do read -rp "Not unlocked. Press Enter to try again. "; done
sleep 3"#;
    Command::new("gnome-terminal")
        .args(["--wait", "--title", "Unlock the home lab", "--"])
        .args(["bash", "-c", script, "unlock"])
        .arg(unlock.canonicalize().unwrap_or(unlock))
        .status()
        .context("open gnome-terminal")?;
    Ok(())
}

pub fn run(_args: Args) -> Result<()> {
    // First, so a PC without a vault or without GNOME exits at once.
    if !is_locked() {
        return Ok(());
    }
    let (mut away, mut prev) = (true, idle_ms()?);
    while is_locked() {
        sleep(POLL);
        let idle = idle_ms()?;
        let (now_away, open) = step(away, prev, idle);
        if open {
            prompt()?;
        }
        (away, prev) = (now_away, idle);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gdbus_output() {
        assert_eq!(parse_idle("(uint64 2,)\n"), Some(2));
        assert_eq!(parse_idle("Error: no such name"), None);
    }

    #[test]
    fn opens_on_the_first_input_after_being_away() {
        // Auto-login at boot: the idle time only grows, so no window.
        assert_eq!(step(true, 1_000, 3_000), (true, false));
        // You come back and move the mouse.
        assert_eq!(step(true, 3_000, 200), (false, true));
        // Still at the PC after closing the window: no second window.
        assert_eq!(step(false, 5_000, 100), (false, false));
        // Away again for 10 minutes, so the next input opens it again.
        assert_eq!(step(false, 599_000, AWAY_MS), (true, false));
        assert_eq!(step(true, AWAY_MS, 50), (false, true));
    }
}

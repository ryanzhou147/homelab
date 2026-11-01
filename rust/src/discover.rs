//! Gets this week's ListenBrainz music recommendations into Navidrome: Explo downloads them into
//! MEDIA/music/explo and makes a playlist of them. The homelab-discover timer runs it each Monday;
//! scripts/setup-explo.sh writes its settings first.

use std::process::Command;

use anyhow::{Result, ensure};

use crate::common::repo_dir;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(_args: Args) -> Result<()> {
    let status = Command::new("docker")
        .args(["compose", "--profile", "discovery", "run", "--rm", "explo"])
        .current_dir(repo_dir())
        .status()?;
    ensure!(status.success(), "explo failed: {status}");
    Ok(())
}

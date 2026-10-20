//! Writes ${DATA}/status/downloads.json: how far the current spotdl run is, for the home page.
//!
//! spotdl reports progress only in its log. The newer of the two fill logs holds the current run,
//! which starts at spotdl's "Processing query: <list file>" line; the list file gives the total.
//! The homelab-download-status timer runs this every minute.

use crate::common::data_dir;
use serde::Serialize;
use std::path::Path;

const DONE: &str = "Downloaded \"";
const SKIPPED: &str = "(file already exists)";
// "no usable results" is only a retry warning; these lines end a song.
const FAILED: [&str; 3] = [
    "LookupError: No results found",
    "AudioProviderError",
    "Error occurred while",
];

#[derive(clap::Args)]
pub struct Args {}

#[derive(Serialize, Debug, PartialEq)]
pub struct Status {
    done: usize,
    failed: usize,
    total: usize,
    left: usize,
    percent: u64,
    last: String,
}

#[derive(Serialize)]
struct Out {
    #[serde(flatten)]
    status: Status,
    running: bool,
}

/// Counts of the last spotdl run in a log.
pub fn status(log_text: &str) -> Status {
    let Some((_, run)) = log_text.rsplit_once("Processing query: ") else {
        return Status {
            done: 0,
            failed: 0,
            total: 0,
            left: 0,
            percent: 0,
            last: String::new(),
        };
    };
    let (list_file, body) = run.split_once('\n').unwrap_or((run, ""));
    let total = std::fs::read_to_string(list_file.trim())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<serde_json::Value>>(&t).ok())
        .map_or(0, |v| v.len());
    // Python's str.splitlines: spotdl's progress output can hold \r as well as \n.
    let lines: Vec<&str> = body
        .split(|c| "\n\r\x0b\x0c\x1c\x1d\x1e\u{85}\u{2028}\u{2029}".contains(c))
        .collect();
    let done: Vec<&str> = lines.iter().copied().filter(|l| l.contains(DONE)).collect();
    let failed = lines
        .iter()
        .filter(|l| FAILED.iter().any(|w| l.contains(w)))
        .count();
    let finished = done.len() + failed + lines.iter().filter(|l| l.contains(SKIPPED)).count();
    let percent = if total > 0 {
        // Python's round() rounds half to even.
        ((100 * finished) as f64 / total as f64)
            .round_ties_even()
            .min(100.0) as u64
    } else {
        0
    };
    Status {
        done: done.len(),
        failed,
        total,
        left: total.saturating_sub(finished),
        percent,
        last: done
            .last()
            .and_then(|l| l.split('"').nth(1))
            .unwrap_or("")
            .to_string(),
    }
}

fn newest_log(data: &Path) -> Option<std::path::PathBuf> {
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for name in ["exported-fill.log", "music-fill.log"] {
        let path = data.join(name);
        let Ok(mtime) = path.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        // Strictly newer: on a tie the first log wins, as with Python's max().
        if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    let data = data_dir();
    let text = match newest_log(&data) {
        Some(log) => String::from_utf8_lossy(&std::fs::read(log)?).into_owned(),
        None => String::new(),
    };
    let running = std::process::Command::new("pgrep")
        .args(["-f", "bin/spotdl download"])
        .output()?
        .status
        .success();
    let out = data.join("status").join("downloads.json");
    std::fs::create_dir_all(data.join("status"))?;
    let tmp = out.with_extension("tmp");
    let status = status(&text);
    std::fs::write(&tmp, serde_json::to_string(&Out { status, running })?)?;
    std::fs::rename(tmp, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_the_last_run() {
        let dir = tempfile::tempdir().unwrap();
        let songs = dir.path().join("list.spotdl");
        std::fs::write(
            &songs,
            serde_json::to_string(&vec![serde_json::json!({}); 5]).unwrap(),
        )
        .unwrap();
        let log = format!(
            "Processing query: /old.spotdl\nDownloaded \"Old - Song\": url\n\
             Processing query: {}   \nDownloaded \"A - One\": url\n\
             YouTube Music returned no usable results for b - two\nafter 3 attempts\n\
             LookupError: No results found for song: b - two\n\
             YouTube Music returned no usable results for c - three\n\
             Downloaded \"C - Three\": url\n\
             Skipping D - Four (file already exists) (duplicate)\n",
            songs.display()
        );
        assert_eq!(
            status(&log),
            Status {
                done: 2,
                failed: 1,
                total: 5,
                left: 1,
                percent: 80,
                last: "C - Three".into()
            }
        );
    }

    #[test]
    fn log_without_a_run() {
        assert_eq!(status("nothing yet").total, 0);
    }
}

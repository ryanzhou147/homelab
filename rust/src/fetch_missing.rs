//! Fetches the songs the Spotify sync found missing: Soulseek first, then online. A song tried in
//! the last RETRY_DAYS is skipped, so the nightly sync does not search for it every night.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::soulseek::{self, Wanted};
use crate::spotify::Track;

const RETRY_DAYS: u64 = 14;

fn today() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400)
}

fn key(track: &Track) -> String {
    format!("{} - {}", track.artists.join(", "), track.title)
}

/// "day<TAB>key" lines: the day (since 1970) each song was last tried.
fn read_tried(path: &Path) -> HashMap<String, u64> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(day, key)| Some((key.to_string(), day.parse().ok()?)))
        .collect()
}

/// The missing tracks to try now: named, once each, and not tried in the last RETRY_DAYS.
fn due<'a>(missing: &'a [Track], tried: &HashMap<String, u64>, today: u64) -> Vec<&'a Track> {
    let mut seen = HashSet::new();
    missing
        .iter()
        .filter(|t| !t.title.trim().is_empty() && seen.insert(key(t)))
        .filter(|t| {
            tried
                .get(&key(t))
                .is_none_or(|day| today >= day + RETRY_DAYS)
        })
        .collect()
}

/// Tries each due track and records it as tried; returns how many were filed.
pub fn run(missing: &[Track], tried_file: &Path) -> Result<usize> {
    let mut tried = read_tried(tried_file);
    let now = today();
    let tracks = due(missing, &tried, now);
    if tracks.is_empty() {
        return Ok(0);
    }
    println!(
        "Fetching {} missing songs (Soulseek, then online)",
        tracks.len()
    );
    let wanted: Vec<Wanted> = tracks
        .iter()
        .map(|t| Wanted {
            title: &t.title,
            artists: &t.artists,
            seconds: (t.song.as_ref())
                .and_then(|s| s["duration"].as_u64())
                .unwrap_or(0),
        })
        .collect();
    let mut log = |line: String| println!("{line}");
    // The sync matches the playlists again after the fetch, so no update is needed on the way.
    crate::online::check()?;
    // Not marked tried when the fetch could not run (slskd stopped by the disk guard).
    let filed = soulseek::fetch(&wanted, &mut log, &mut |_| true)?;
    for t in &tracks {
        tried.insert(key(t), now);
    }
    let lines: String = tried.iter().map(|(k, d)| format!("{d}\t{k}\n")).collect();
    std::fs::write(tried_file, lines)?;
    Ok(filed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_unnamed_repeated_and_recently_tried_tracks() {
        let t = |title: &str| Track::new(title, vec!["A".into()], "");
        let missing = [t("Old"), t("New"), t("New"), t(" "), t("Recent")];
        let tried = HashMap::from([
            ("A - Old".to_string(), 100),
            ("A - Recent".to_string(), 110),
        ]);
        let titles: Vec<&str> = due(&missing, &tried, 114)
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["Old", "New"]);
    }

    #[test]
    fn reads_what_it_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fetch-tried.txt");
        std::fs::write(&path, "20000\tA, B - Song\nbad line\n").unwrap();
        assert_eq!(
            read_tried(&path),
            HashMap::from([("A, B - Song".to_string(), 20000)])
        );
    }
}

//! Finds single songs on Soulseek through slskd, downloads the best match, and files it into the
//! music library under "Soulseek singles", where Navidrome finds it. Lidarr works by album; this
//! is for songs that come without an album, such as a pasted playlist link.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::common::media_dir;
use crate::navidrome::agent;

const API: &str = "http://127.0.0.1:5030/slskd/api/v0";
const AUDIO: [&str; 5] = ["flac", "mp3", "m4a", "ogg", "opus"];
const MIN_MP3_KBPS: u64 = 192;
const MAX_LENGTH_GAP_S: u64 = 15;
// Searches are spaced out so the Soulseek server does not throttle the account.
const SEARCH_GAP: Duration = Duration::from_secs(8);
const SEARCH_WAIT: Duration = Duration::from_secs(45);
const DOWNLOAD_WAIT: Duration = Duration::from_secs(30 * 60);
const NOT_RUNNING: &str = "slskd is not running";
// The playlist is saved again after this many new songs, or after this long with any.
const UPDATE_EVERY: usize = 5;
const UPDATE_GAP: Duration = Duration::from_secs(5 * 60);
static NOT_WORDS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\(.*?\)|\[.*?\]|\b(?:feat|ft)\..*|\s-\s.*|\S*\*\S*").unwrap()
});
static NOT_SEARCHABLE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[^\w\s']").unwrap());

/// A song to look for: title, artists and length in seconds (0 when unknown).
pub struct Wanted<'a> {
    pub title: &'a str,
    pub artists: &'a [String],
    pub seconds: u64,
}

/// Lowercase words of two characters or more, accents removed (other scripts stay), without
/// (..), [..], "feat." parts or words with a '*' in them (a censored "Sh*t" never matches the
/// uncensored file name).
fn words(s: &str) -> Vec<String> {
    tokens(&NOT_WORDS.replace_all(s, " "))
}

/// Every lowercase word of two characters or more, accents removed. For a peer's file path,
/// where removing parts could only remove the title.
fn tokens(s: &str) -> Vec<String> {
    let folded: String = s
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 1)
        .map(String::from)
        .collect()
}

/// Higher is better; None means the file is not this song.
fn score(song: &Wanted, file: &Value) -> Option<f64> {
    let name = file["filename"].as_str()?.replace('\\', "/");
    let base = name.rsplit('/').next().unwrap_or(&name);
    let ext = base.rsplit_once('.').map(|(_, e)| e.to_lowercase())?;
    if !AUDIO.contains(&ext.as_str()) {
        return None;
    }
    let path_words: std::collections::HashSet<String> = tokens(&name).into_iter().collect();
    let title = words(song.title);
    if title.is_empty() || !title.iter().all(|w| path_words.contains(w)) {
        return None;
    }
    let artist: Vec<String> = song.artists.iter().flat_map(|a| words(a)).collect();
    if !artist.is_empty() && !artist.iter().any(|w| path_words.contains(w)) {
        return None;
    }
    let kbps = file["bitRate"].as_u64().unwrap_or(0);
    if ext == "mp3" && kbps < MIN_MP3_KBPS {
        return None;
    }
    let have = file["length"].as_u64().unwrap_or(0);
    if song.seconds > 0 && have > 0 && song.seconds.abs_diff(have) > MAX_LENGTH_GAP_S {
        return None;
    }
    let quality = if ext == "flac" {
        3.0
    } else if kbps >= 320 {
        2.0
    } else {
        1.0
    };
    let extra = tokens(base)
        .iter()
        .filter(|w| !title.contains(w) && !artist.contains(w))
        .count();
    Some(quality - extra as f64 * 0.05)
}

/// The best file among the users with a free upload slot: (user, file).
fn best_match(song: &Wanted, responses: &[Value]) -> Option<(String, Value)> {
    let mut picks: Vec<(f64, u64, String, Value)> = Vec::new();
    for r in responses.iter().filter(|r| r["hasFreeUploadSlot"] == true) {
        for f in r["files"].as_array().into_iter().flatten() {
            if let Some(s) = score(song, f) {
                let user = r["username"].as_str().unwrap_or_default().to_string();
                picks.push((s, r["uploadSpeed"].as_u64().unwrap_or(0), user, f.clone()));
            }
        }
    }
    picks.sort_by(|a, b| {
        (b.0, b.1)
            .partial_cmp(&(a.0, a.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    picks.into_iter().next().map(|(_, _, u, f)| (u, f))
}

fn call(method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let req = ureq::http::Request::builder()
        .method(method)
        .uri(format!("{API}{path}"))
        .header("Content-Type", "application/json")
        .body(body.map(|b| b.to_string()).unwrap_or_default())?;
    let mut resp = agent()
        .run(req)
        .with_context(|| format!("slskd {method} {path}"))?;
    let status = resp.status();
    let text = resp.body_mut().read_to_string()?;
    // The fetch gate answers 502 when slskd behind it is down.
    if status == 502 {
        bail!(
            "{NOT_RUNNING}; the disk guard stops it below 20 GB free (start it with: docker compose start slskd soularr)"
        );
    }
    if !status.is_success() {
        bail!("slskd {method} {path}: HTTP {status}");
    }
    Ok(if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text)?
    })
}

fn search(text: &str) -> Result<Vec<Value>> {
    let id = call("POST", "/searches", Some(json!({"searchText": text})))?["id"]
        .as_str()
        .context("slskd gave no search id")?
        .to_string();
    let start = Instant::now();
    while start.elapsed() < SEARCH_WAIT {
        sleep(Duration::from_secs(3));
        if call("GET", &format!("/searches/{id}"), None)?["isComplete"] == true {
            break;
        }
    }
    let responses = call("GET", &format!("/searches/{id}/responses"), None)?;
    let _ = call("DELETE", &format!("/searches/{id}"), None);
    Ok(responses.as_array().cloned().unwrap_or_default())
}

/// Where slskd puts a remote file: its last folder and its name under the download folder.
/// Empty, "." and ".." parts are left out, so a peer's name cannot point elsewhere.
fn local_path(downloads: &Path, remote: &str) -> PathBuf {
    let parts: Vec<&str> = remote
        .split(['\\', '/'])
        .filter(|p| !matches!(*p, "" | "." | ".."))
        .collect();
    let folder = if parts.len() > 1 {
        parts[parts.len() - 2]
    } else {
        ""
    };
    downloads.join(folder).join(parts.last().unwrap_or(&""))
}

fn safe(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    if t.trim().is_empty() {
        "Unknown".into()
    } else {
        t.trim().to_string()
    }
}

/// What to ask Soulseek: the main artist and the title without Spotify's "(with ...)",
/// "[...]" or "feat." parts, which files rarely carry and which make the search find nothing.
fn search_text(artist: &str, title: &str) -> String {
    let title = NOT_WORDS.replace_all(title, " ");
    let text = NOT_SEARCHABLE
        .replace_all(&format!("{artist} {title}"), " ")
        .into_owned();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether to save the playlist again: some new songs, and enough of them, enough time, or the end.
fn due(new_songs: usize, since_last: Duration, end: bool) -> bool {
    new_songs > 0 && (end || new_songs >= UPDATE_EVERY || since_last >= UPDATE_GAP)
}

/// Runs when new songs are in the library; it gets the job's log.
pub type OnFiled<'a> = dyn FnMut(&mut dyn FnMut(String)) + 'a;

/// Moves the finished downloads among `queued` into the library; returns how many it moved.
/// slskd keeps unfinished files in its own incomplete folder, so a file in `downloads` is done.
fn file_finished(queued: &mut Vec<(String, String)>, log: &mut dyn FnMut(String)) -> usize {
    let downloads = media_dir().join("downloads/slskd");
    let library = media_dir().join("music/Soulseek singles");
    let mut filed = 0;
    queued.retain(|(artist, remote)| {
        let src = local_path(&downloads, remote);
        if !src.exists() {
            return true;
        }
        // shortcut: a Soularr download with the same folder and file name would be taken too;
        // rare, so not tracked by transfer id.
        let dest = library.join(safe(artist));
        let name = src.file_name().unwrap_or_default().to_owned();
        if dest.join(&name).exists() {
            log(format!(
                "already in the library: {}",
                name.to_string_lossy()
            ));
            return false;
        }
        match std::fs::create_dir_all(&dest).and_then(|()| std::fs::rename(&src, dest.join(&name)))
        {
            Ok(()) => {
                filed += 1;
                log(format!("filed: {}", name.to_string_lossy()));
            }
            Err(e) => log(format!("could not file {}: {e}", src.display())),
        }
        false
    });
    filed
}

/// Searches, downloads and files each song; returns how many were filed. Songs go into the
/// library as their downloads finish, and `on_filed` runs after every few of them (and at the
/// end), so the playlist fills while the search goes on. `log` gets one line per song. A failed
/// search or download skips that song; a stopped slskd stops the job.
pub fn fetch(
    songs: &[Wanted],
    log: &mut dyn FnMut(String),
    on_filed: &mut OnFiled,
) -> Result<usize> {
    let mut queued = Vec::new();
    let (mut filed, mut unannounced, mut last_update) = (0, 0, Instant::now());
    let mut collect =
        |queued: &mut Vec<(String, String)>, log: &mut dyn FnMut(String), force: bool| {
            let new = file_finished(queued, log);
            filed += new;
            unannounced += new;
            if due(unannounced, last_update.elapsed(), force) {
                on_filed(log);
                unannounced = 0;
                last_update = Instant::now();
            }
        };
    for (i, song) in songs.iter().enumerate() {
        let artist = song.artists.first().map_or("", String::as_str);
        let text = search_text(artist, song.title);
        let n = format!("[{}/{}]", i + 1, songs.len());
        match search(&text).map(|r| best_match(song, &r)) {
            Ok(Some((user, file))) => {
                let body = json!([{"filename": file["filename"], "size": file["size"]}]);
                let path = format!("/transfers/downloads/{}", crate::text::quote(&user, ""));
                let remote = file["filename"].as_str().unwrap_or("").to_string();
                match call("POST", &path, Some(body)) {
                    Ok(_) => {
                        log(format!("{n} downloading: {text}"));
                        queued.push((artist.to_string(), remote));
                    }
                    // An earlier job queued the same file; it still arrives, so wait for it.
                    Err(e) if e.to_string().contains("HTTP 409") => {
                        log(format!("{n} already downloading: {text}"));
                        queued.push((artist.to_string(), remote));
                    }
                    Err(e) => log(format!("{n} could not queue {text}: {e}")),
                }
            }
            Ok(None) => log(format!("{n} not found on Soulseek: {text}")),
            Err(e) if e.to_string().starts_with(NOT_RUNNING) => {
                collect(&mut queued, log, true);
                return Err(e);
            }
            Err(e) => log(format!("{n} search failed for {text}: {e}")),
        }
        sleep(SEARCH_GAP);
        collect(&mut queued, log, false);
    }
    let start = Instant::now();
    while !queued.is_empty() && start.elapsed() < DOWNLOAD_WAIT {
        sleep(Duration::from_secs(20));
        collect(&mut queued, log, false);
    }
    collect(&mut queued, log, true);
    for (_, remote) in &queued {
        log(format!(
            "still downloading after 30 min, left out: {remote}"
        ));
    }
    Ok(filed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song<'a>(artists: &'a [String]) -> Wanted<'a> {
        Wanted {
            title: "Third Eye Sh*t",
            artists,
            seconds: 200,
        }
    }

    #[test]
    fn scores_like_the_python_check() {
        let artists = vec!["Joey Bada$$".to_string()];
        let s = song(&artists);
        let good = json!({"filename": r"Music\Joey Bada$$ - 1999\05 Third Eye Shit.mp3", "bitRate": 320, "length": 201});
        let low = json!({"filename": r"Music\Joey Bada$$ - 1999\05 Third Eye Shit.mp3", "bitRate": 128, "length": 201});
        let wrong = json!({"filename": r"Music\Joey Bada$$\Christ Conscious.mp3", "bitRate": 320, "length": 200});
        let flac =
            json!({"filename": r"Music\Joey Bada$$ - 1999\05 Third Eye Shit.flac", "length": 201});
        let long = json!({"filename": r"Music\Joey Bada$$ - 1999\05 Third Eye Shit.mp3", "bitRate": 320, "length": 400});
        assert!(score(&s, &good).is_some());
        assert!(
            score(&s, &low).is_none() && score(&s, &wrong).is_none() && score(&s, &long).is_none()
        );
        assert!(score(&s, &flac).unwrap() > score(&s, &good).unwrap());
        let resp = vec![
            json!({"username": "a", "hasFreeUploadSlot": true, "uploadSpeed": 1, "files": [good]}),
            json!({"username": "b", "hasFreeUploadSlot": false, "uploadSpeed": 9, "files": [flac]}),
        ];
        assert_eq!(best_match(&s, &resp).unwrap().0, "a");
    }

    #[test]
    fn matches_titles_in_other_scripts() {
        let artists = vec!["YOASOBI".to_string()];
        let s = Wanted {
            title: "夜に駆ける",
            artists: &artists,
            seconds: 0,
        };
        let file = json!({"filename": r"Music\YOASOBI\01 夜に駆ける.flac"});
        assert!(score(&s, &file).is_some());
        assert_eq!(words("Beyoncé (Live)"), ["beyonce"]);
        let gift = Wanted {
            title: "Gift",
            artists: &artists,
            seconds: 0,
        };
        assert!(score(&gift, &json!({"filename": r"Music\YOASOBI\05 Gift.flac"})).is_some());
        let take_care = Wanted {
            title: "Take Care",
            artists: &artists,
            seconds: 0,
        };
        let feat =
            json!({"filename": r"Music\YOASOBI feat. Rihanna - Take Care.mp3", "bitRate": 320});
        assert!(score(&take_care, &feat).is_some());
        assert_eq!(words("Defeat (feat. X)"), ["defeat"]);
    }

    #[test]
    fn searches_for_the_main_artist_and_title() {
        let text = search_text("The Beatles", "Hey Jude - Remastered 2015");
        assert_eq!(text, "The Beatles Hey Jude");
        let text = search_text("JID", "Wholeheartedly (with Ty Dolla $ign & 6LACK)");
        assert_eq!(text, "JID Wholeheartedly");
        assert_eq!(
            search_text("Alesso", "Heroes (we could be)"),
            "Alesso Heroes"
        );
        assert_eq!(search_text("AC/DC", "T.N.T. - Remastered"), "AC DC T N T");
    }

    #[test]
    fn updates_the_playlist_every_few_songs_or_minutes() {
        let minute = Duration::from_secs(60);
        assert!(!due(0, 10 * minute, true));
        assert!(!due(2, minute, false));
        assert!(due(2, minute, true) && due(5, minute, false) && due(1, 5 * minute, false));
    }

    #[test]
    fn maps_remote_paths_and_names() {
        let p = local_path(Path::new("/d"), r"@@x\Music\Album\01 Song.flac");
        assert_eq!(p, Path::new("/d/Album/01 Song.flac"));
        assert_eq!(
            local_path(Path::new("/d"), r"x\..\Song.mp3"),
            Path::new("/d/x/Song.mp3")
        );
        assert_eq!(safe("AC/DC: Live?"), "AC_DC_ Live_");
        assert_eq!(safe("  "), "Unknown");
    }
}

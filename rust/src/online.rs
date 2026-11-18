//! Gets single songs that Soulseek lacks from YouTube Music, YouTube and SoundCloud through
//! yt-dlp, and files them under "Online singles". The audio is lossy (about 128 kbps AAC on
//! YouTube), so Soulseek comes first.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::common::{data_dir, media_dir, setting};
use crate::soulseek::{Wanted, is_song, safe, write_tags};

// Best catalog first: YouTube Music lists the label's own tracks with their real artists. Plain
// YouTube and SoundCloud add the uploads (edits, small artists) that no catalog has.
const SEARCHES: [&str; 3] = [
    "https://music.youtube.com/search?q={}#songs",
    "ytsearch5:{}",
    "scsearch5:{}",
];
// m4a and mp3 need no ffmpeg to convert, and Navidrome reads both.
const FORMAT: &str = "bestaudio[ext=m4a]/bestaudio[ext=mp3]";

/// The lab's own yt-dlp (install-homelab.sh puts it in $DATA/bin, where -U can update it),
/// else YT_DLP, else the one on PATH.
fn yt_dlp() -> PathBuf {
    let own = data_dir().join("bin/yt-dlp");
    setting("YT_DLP")
        .map(PathBuf::from)
        .unwrap_or(if own.exists() { own } else { "yt-dlp".into() })
}

fn run(args: &[&str]) -> Result<String> {
    let out = Command::new(yt_dlp())
        .args(["--no-update", "--no-warnings"])
        .args(args)
        .output()
        .context("run yt-dlp")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("yt-dlp: {}", err.lines().last().unwrap_or("failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn json(args: &[&str]) -> Result<Value> {
    Ok(serde_json::from_str(&run(args)?)?)
}

/// Fails when yt-dlp does not run, so a broken copy is not taken for "every song is missing".
pub fn check() -> Result<()> {
    run(&["--version"]).map(drop)
}

/// Updates the lab's own copy: YouTube changes often, and an old yt-dlp stops finding songs.
pub fn update(log: &mut dyn FnMut(String)) {
    let own = data_dir().join("bin/yt-dlp");
    if own.exists()
        && let Err(e) = Command::new(&own).arg("-U").output()
    {
        log(format!("could not update yt-dlp: {e}"));
    }
}

/// Whether a search result is the song: its name, its people and its length.
fn fits(song: &Wanted, info: &Value) -> bool {
    let text = |k: &str| info[k].as_str().unwrap_or("");
    let name = if text("track").is_empty() {
        text("title")
    } else {
        text("track")
    };
    let artists = (info["artists"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let who = [
        &artists,
        text("artist"),
        text("channel"),
        text("uploader"),
        text("title"),
    ]
    .join(" ");
    let seconds = info["duration"].as_f64().map_or(0, |d| d.round() as u64);
    is_song(song, name, &who, seconds)
}

/// The first search result that is the song, as a URL. An error only when every search failed,
/// so a yt-dlp that YouTube blocks or that does not run is not taken for "not found".
fn find(song: &Wanted, text: &str) -> Result<Option<String>> {
    let (mut answered, mut error) = (false, None);
    for search in SEARCHES {
        let query = if search.starts_with("http") {
            search.replace("{}", &crate::text::quote(text, ""))
        } else {
            search.replace("{}", text)
        };
        let list = match json(&["-J", "--flat-playlist", "--playlist-end", "3", &query]) {
            Ok(list) => list,
            Err(e) => {
                error = Some(e);
                continue;
            }
        };
        answered = true;
        for entry in list["entries"].as_array().into_iter().flatten() {
            let Some(url) = entry["url"].as_str() else {
                continue;
            };
            // Flat YouTube Music results carry only a title: read the page for artist and length.
            let info = if entry["duration"].is_null() {
                json(&["-J", "--skip-download", url]).unwrap_or(Value::Null)
            } else {
                entry.clone()
            };
            if fits(song, &info) {
                return Ok(Some(url.to_string()));
            }
        }
    }
    match error {
        Some(e) if !answered => Err(e),
        _ => Ok(None),
    }
}

/// Finds, downloads, tags and files the song; true when it is now in the library.
pub fn get(song: &Wanted, text: &str, log: &mut dyn FnMut(String)) -> Result<bool> {
    let Some(url) = find(song, text)? else {
        return Ok(false);
    };
    let downloads = media_dir().join("downloads/online");
    let template = downloads.join("%(id)s.%(ext)s");
    let printed = run(&[
        "-f",
        FORMAT,
        "-o",
        &template.to_string_lossy(),
        "--print",
        "after_move:filepath",
        &url,
    ])?;
    let src = PathBuf::from(printed.trim());
    if !src.starts_with(&downloads) || !src.is_file() {
        bail!("yt-dlp gave no file for {url}");
    }
    // Tagged before the move, so a write that fails halfway never harms a library file.
    write_tags(&src, song)?;
    let artist = song.artists.first().map_or("", String::as_str);
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dest = file_into(&media_dir(), artist, song.title, &ext);
    if dest.exists() {
        std::fs::remove_file(&src)?;
    } else {
        std::fs::create_dir_all(dest.parent().context("no folder")?)?;
        std::fs::rename(&src, &dest)?;
    }
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    log(format!("filed: {name} (from {url})"));
    Ok(true)
}

fn file_into(media: &Path, artist: &str, title: &str, ext: &str) -> PathBuf {
    let name = format!("{}.{ext}", safe(title));
    media
        .join("music/Online singles")
        .join(safe(artist))
        .join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fits_the_label_track_and_not_a_cover() {
        let artists = ["Killsaka".to_string()];
        let song = Wanted {
            title: "Too Many Chances",
            artists: &artists,
            seconds: 187,
        };
        let label = json!({"track": "Too Many Chances", "artists": ["Killsaka"], "duration": 187});
        let other = json!({"track": "Too Many Chances (feat. NYF)", "artists": ["Kbeatc", "NYF"],
            "channel": "KBeatc Music", "duration": 223});
        let upload =
            json!({"title": "Killsaka - Too Many Chances", "channel": "beats", "duration": 189.4});
        let slowed = json!({"title": "Killsaka - Too Many Chances (slowed)", "duration": 188});
        assert!(fits(&song, &label) && fits(&song, &upload));
        assert!(!fits(&song, &other) && !fits(&song, &slowed));
        // A cover: the title has the artist's "the", so "the" must not count as the artist.
        let spots = ["The Ink Spots".to_string()];
        let fire = Wanted {
            title: "I Don't Want to Set the World on Fire",
            artists: &spots,
            seconds: 181,
        };
        let heidt = json!({"title": "I Don't Want to Set the World on Fire", "channel": "Horace Heidt", "duration": 184});
        let real = json!({"title": "The Ink Spots - I Don't Want to Set the World on Fire", "duration": 181});
        assert!(!fits(&fire, &heidt) && fits(&fire, &real));
    }

    #[test]
    fn files_by_artist_and_title() {
        let path = file_into(Path::new("/m"), "AC/DC", "T.N.T.", "m4a");
        assert_eq!(path, Path::new("/m/music/Online singles/AC_DC/T.N.T..m4a"));
    }
}

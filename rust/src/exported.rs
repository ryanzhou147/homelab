//! Writes a spotdl list of the songs in Exportify CSV playlists that are not in Navidrome.
//!
//! The song details come from the CSV, because Spotify blocks track lookups for this app. Songs
//! in spotdl-tried.txt are left out; the caller adds the list's URLs there after spotdl.

use crate::common::{data_dir, load_env};
use crate::import::{artists_of, csv_files, csv_rows, field, server_and_user};
use crate::navidrome::Navidrome;
use crate::spotify::Track;
use crate::sync::write_missing_songs;
use crate::text::die;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// Folder with Exportify CSV files.
    folder: Option<PathBuf>,
    /// The .spotdl file to write.
    out: Option<PathBuf>,
}

/// Python's int(text), which allows spaces around the digits.
fn int(text: &str) -> Result<i64> {
    text.trim()
        .parse()
        .with_context(|| format!("not a number: {text:?}"))
}

/// spotdl's Song fields from one Exportify row. The CSV has no track number or cover.
fn song_from_row(row: &HashMap<String, String>) -> Result<Value> {
    let artists: Vec<&str> = field(row, "Artist Name(s)")
        .split(';')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .collect();
    let first = artists.first().copied().unwrap_or("");
    let uri = field(row, "Track URI");
    let track_id = uri.rsplit_once(':').map_or(uri, |(_, id)| id);
    let date = match field(row, "Release Date") {
        "" => "0",
        d => d,
    };
    let ms = match field(row, "Duration (ms)") {
        "" => 0,
        ms => int(ms)?,
    };
    let isrc = match field(row, "ISRC") {
        "" => Value::Null,
        isrc => json!(isrc),
    };
    let url = if track_id.is_empty() {
        String::new()
    } else {
        format!("https://open.spotify.com/track/{track_id}")
    };
    Ok(json!({
        "name": field(row, "Track Name"), "artists": artists, "artist": first, "genres": [],
        "disc_number": 1, "disc_count": 1, "album_name": field(row, "Album Name"),
        "album_artist": first,
        // Python's round() rounds half to even.
        "duration": (ms as f64 / 1000.0).round_ties_even() as i64,
        "year": int(&date.chars().take(4).collect::<String>())?,
        "date": date, "track_number": 1, "tracks_count": 1, "song_id": track_id,
        "explicit": field(row, "Explicit") == "true", "publisher": field(row, "Record Label"),
        "url": url, "isrc": isrc, "cover_url": null, "copyright_text": null,
        "popularity": null, "album_id": null, "artist_id": null, "album_type": null,
    }))
}

pub fn run(args: Args) -> Result<()> {
    let (Some(folder), Some(out)) = (args.folder, args.out) else {
        die("Usage: homelab exported-missing <folder with Exportify CSV files> <out.spotdl>")
    };
    let data = data_dir();
    let password = load_env(&data.join("secrets/navidrome.env"))
        .get("NAVIDROME_PASSWORD")
        .cloned()
        .unwrap_or_default();
    let (server, user) = server_and_user();
    let navidrome = Navidrome::new(&server, &user, &password, "spotify-import");
    navidrome.call("ping", &[])?;
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    for path in csv_files(&folder) {
        // Episode rows stay: in these exports they are unreleased songs uploaded as podcasts.
        let rows = csv_rows(&path)?;
        let rows = rows
            .iter()
            .filter(|r| !field(r, "Track Name").is_empty() && !field(r, "Track URI").is_empty());
        for row in rows {
            if !seen.insert(field(row, "Track URI").to_string()) {
                continue;
            }
            let artists = artists_of(field(row, "Artist Name(s)"));
            let track = Track::new(field(row, "Track Name"), artists, field(row, "Album Name"));
            if navidrome.find(&track)?.is_none() {
                missing.push(song_from_row(row)?);
            }
        }
    }
    let count = write_missing_songs(
        missing.iter().map(Some),
        &out,
        &data.join("spotdl-tried.txt"),
    )?;
    println!(
        "{} songs not in the library; {count} not tried before, written to {}",
        missing.len(),
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn song_from_exportify_row_has_what_spotdl_needs() {
        let song = song_from_row(&row(&[
            ("Track URI", "spotify:track:abc123"),
            ("Track Name", "We Found Love"),
            ("Artist Name(s)", "Rihanna;Calvin Harris"),
            ("Album Name", "Talk That Talk"),
            ("Release Date", "2011-11-18"),
            ("Duration (ms)", "215226"),
            ("ISRC", "USUM71111621"),
        ]))
        .unwrap();
        assert_eq!(song["url"], "https://open.spotify.com/track/abc123");
        assert_eq!(song["artists"], json!(["Rihanna", "Calvin Harris"]));
        assert_eq!(song["album_artist"], "Rihanna");
        assert_eq!(
            (&song["duration"], &song["year"], &song["isrc"]),
            (&json!(215), &json!(2011), &json!("USUM71111621"))
        );
    }

    #[test]
    fn year_only_release_date() {
        let song = song_from_row(&row(&[
            ("Track URI", "spotify:track:x"),
            ("Release Date", "1994"),
        ]));
        assert_eq!(song.unwrap()["year"], 1994);
    }
}

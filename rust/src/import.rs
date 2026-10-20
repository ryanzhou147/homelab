//! Rebuilds Spotify playlists (Exportify CSV exports) as Navidrome playlists. Port of
//! scripts/import-spotify-playlists.py.
//!
//! Each track is matched to a library song by title and artist. Tracks not in the library go to
//! missing-albums.csv, a shopping list by album with a Bandcamp search link. Asks for the
//! password unless NAVIDROME_PASSWORD is set; nothing is stored.

use crate::navidrome::Navidrome;
use crate::spotify::Track;
use crate::text::{die, most_common, quote_plus};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const DEFAULT_SERVER: &str = "https://your-pc.your-tailnet.ts.net/music";
const SHOPPING_LIST: &str = "missing-albums.csv";

#[derive(clap::Args)]
pub struct Args {
    /// Folder with Exportify CSV files.
    folder: Option<PathBuf>,
}

/// NAVIDROME_URL and NAVIDROME_USER, with the import's defaults.
pub fn server_and_user() -> (String, String) {
    let server = std::env::var("NAVIDROME_URL").unwrap_or_else(|_| DEFAULT_SERVER.into());
    let user = std::env::var("NAVIDROME_USER").unwrap_or_else(|_| "admin".into());
    (server, user)
}

/// Exportify separates artists with ';' (older exports used ', ').
pub fn artists_of(field: &str) -> Vec<String> {
    let separator = if field.contains(';') { ";" } else { ", " };
    field
        .split(separator)
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(String::from)
        .collect()
}

/// The *.csv files of a folder in name order; a folder that cannot be read has none.
pub fn csv_files(folder: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".csv"))
        })
        .collect();
    files.sort();
    files
}

/// The rows of a CSV file as header to value maps, like csv.DictReader with utf-8-sig.
pub fn csv_rows(path: &Path) -> Result<Vec<HashMap<String, String>>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers = reader.headers()?.clone();
    reader
        .records()
        .map(|record| {
            let record = record?;
            Ok(headers
                .iter()
                .zip(record.iter())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect())
        })
        .collect()
}

pub fn field<'a>(row: &'a HashMap<String, String>, key: &str) -> &'a str {
    row.get(key).map_or("", String::as_str)
}

fn read_playlist(path: &Path) -> Result<Vec<Track>> {
    Ok(csv_rows(path)?
        .iter()
        .filter(|r| !field(r, "Track Name").is_empty())
        .map(|r| {
            let artists = artists_of(field(r, "Artist Name(s)"));
            Track::new(field(r, "Track Name"), artists, field(r, "Album Name"))
        })
        .collect())
}

/// Creates the playlist, or replaces the songs of the one with the same name.
fn save_playlist(navidrome: &Navidrome, name: &str, song_ids: &[String]) -> Result<()> {
    let existing = navidrome.playlist_id(name)?;
    let mut params = match &existing {
        Some(id) => vec![("playlistId", id.as_str())],
        None => vec![("name", name)],
    };
    params.extend(song_ids.iter().map(|s| ("songId", s.as_str())));
    navidrome.call("createPlaylist", &params)?;
    Ok(())
}

fn write_shopping_list(missing: &[((String, String), usize)], folder: &Path) -> Result<PathBuf> {
    let path = folder.join(SHOPPING_LIST);
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::CRLF)
        .from_path(&path)?;
    writer.write_record(["songs you need", "artist", "album", "bandcamp search"])?;
    for ((artist, album), count) in missing {
        let query = quote_plus(&format!("{artist} {album}"));
        let link = format!("https://bandcamp.com/search?q={query}&item_type=a");
        writer.write_record([&count.to_string(), artist, album, &link])?;
    }
    writer.flush()?;
    Ok(path)
}

/// getpass.getpass: the prompt and the answer on the terminal, with echo off.
fn getpass(prompt: &str) -> Result<String> {
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    let stty = |arg: &str| -> Result<()> {
        let tty_in = std::fs::File::open("/dev/tty")?;
        std::process::Command::new("stty")
            .arg(arg)
            .stdin(tty_in)
            .status()?;
        Ok(())
    };
    write!(tty, "{prompt}")?;
    stty("-echo")?;
    let mut line = String::new();
    let read = std::io::BufReader::new(&tty).read_line(&mut line);
    stty("echo")?;
    writeln!(tty)?;
    read?;
    Ok(line.strip_suffix('\n').unwrap_or(&line).to_string())
}

pub fn run(args: Args) -> Result<()> {
    let folder = match args.folder {
        Some(f) if f.is_dir() => f,
        _ => die("Usage: homelab import-playlists <folder with Exportify CSV files>"),
    };
    let files: Vec<PathBuf> = csv_files(&folder)
        .into_iter()
        .filter(|f| f.file_name().is_some_and(|n| n != SHOPPING_LIST))
        .collect();
    if files.is_empty() {
        die(&format!("No Exportify CSV files in {}.", folder.display()));
    }
    let (server, user) = server_and_user();
    let password = match std::env::var("NAVIDROME_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => getpass(&format!("Password for {user}: "))?,
    };
    let navidrome = Navidrome::new(&server, &user, &password, "spotify-import");
    navidrome.call("ping", &[])?;

    let mut missing = Vec::new();
    for path in &files {
        let tracks = read_playlist(path)?;
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let spaced = stem.replace('_', " ");
        let name = match spaced.trim() {
            "" => stem.as_ref(),
            name => name,
        };
        if tracks.is_empty() {
            println!("{name}: empty, skipped");
            continue;
        }
        let found = tracks
            .iter()
            .map(|t| navidrome.find(t))
            .collect::<Result<Vec<_>>>()?;
        let mut seen = HashSet::new();
        let song_ids: Vec<String> = found
            .iter()
            .flatten()
            .filter(|id| seen.insert(id.as_str()))
            .cloned()
            .collect();
        for (track, song_id) in tracks.iter().zip(&found) {
            if song_id.is_none() {
                let artist = track.artists.first().cloned().unwrap_or_default();
                missing.push((artist, track.album.clone()));
            }
        }
        save_playlist(&navidrome, name, &song_ids)?;
        println!(
            "{name}: {} of {} songs in your library",
            song_ids.len(),
            tracks.len()
        );
    }

    if !missing.is_empty() {
        let total = missing.len();
        let by_album = most_common(missing);
        let shopping = write_shopping_list(&by_album, &folder)?;
        println!("\n{total} songs are not in your library yet. Albums that fill the most:");
        for ((artist, album), count) in by_album.iter().take(10) {
            println!("  {count:3}  {artist} - {album}");
        }
        println!("Full list with Bandcamp links: {}", shopping.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artists_split_on_semicolon_or_old_comma() {
        assert_eq!(
            artists_of("Rihanna;Calvin Harris"),
            ["Rihanna", "Calvin Harris"]
        );
        assert_eq!(artists_of("Tyler, The Creator"), ["Tyler", "The Creator"]);
        assert_eq!(
            artists_of("Tyler, The Creator;Kali Uchis"),
            ["Tyler, The Creator", "Kali Uchis"]
        );
    }

    #[test]
    fn shopping_list_is_most_needed_first_with_bandcamp_links() {
        let dir = tempfile::tempdir().unwrap();
        let missing = most_common([
            ("A".to_string(), "X".to_string()),
            ("B, C".to_string(), "Y & Z".to_string()),
            ("B, C".to_string(), "Y & Z".to_string()),
        ]);
        let path = write_shopping_list(&missing, dir.path()).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(
            text,
            "songs you need,artist,album,bandcamp search\r\n\
             2,\"B, C\",Y & Z,https://bandcamp.com/search?q=B%2C+C+Y+%26+Z&item_type=a\r\n\
             1,A,X,https://bandcamp.com/search?q=A+X&item_type=a\r\n"
        );
    }

    /// Parity check against the script, read-only: prints the matched id of every CSV row.
    /// Run with EXPORT_DIR, NAVIDROME_URL and NAVIDROME_PASSWORD set and `-- --ignored`.
    #[test]
    #[ignore]
    fn live_match_ids() {
        let (server, user) = server_and_user();
        let password = std::env::var("NAVIDROME_PASSWORD").unwrap();
        let navidrome = Navidrome::new(&server, &user, &password, "spotify-import");
        let folder = PathBuf::from(std::env::var("EXPORT_DIR").unwrap());
        for path in csv_files(&folder) {
            for (i, track) in read_playlist(&path).unwrap().iter().enumerate() {
                let id = navidrome.find(track).unwrap();
                let name = path.file_name().unwrap().to_string_lossy();
                println!("{name}\t{i}\t{}", id.as_deref().unwrap_or("-"));
            }
        }
    }
}

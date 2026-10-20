//! Syncs Spotify playlists and Liked Songs to Navidrome and queues missing albums in Lidarr.
//! Port of scripts/sync-spotify.py.
//!
//! One-time OAuth setup (writes $DATA/secrets/spotify.env): `homelab sync-spotify --auth`.
//! It needs SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET of an app with the redirect URI
//! http://127.0.0.1:18080/callback, and saves SPOTIFY_REFRESH_TOKEN in the same file.

use crate::common::{data_dir, load_env};
use crate::lidarr::Lidarr;
use crate::navidrome::Navidrome;
use crate::spotify::{
    self, CachedSpotify, HttpStatus, LIKED_ID, LIKED_NAME, RateLimited, Source, SpotifyClient,
    Track, truthy,
};
use crate::text::{die, most_common, py_repr};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// Parity checks only: set to print each track's matched song id on stderr.
const DUMP_IDS_ENV: &str = "HOMELAB_DUMP_IDS";

#[derive(clap::Args)]
pub struct Args {
    /// Run the one-time Spotify OAuth setup and exit.
    #[arg(long)]
    auth: bool,
    /// Match tracks but do not write playlists or add to Lidarr.
    #[arg(long)]
    dry_run: bool,
    /// Skip adding missing artists/albums to Lidarr.
    #[arg(long)]
    no_lidarr: bool,
    /// Write missing tracks not yet tried by spotdl to this .spotdl file.
    #[arg(long)]
    missing_out: Option<PathBuf>,
}

/// Where matched tracks go: Navidrome, or a fake in the tests.
pub trait Library {
    fn find_track(&self, track: &Track) -> Result<Option<String>>;
    fn save_playlist(&mut self, name: &str, song_ids: &[String]) -> Result<()>;
}

impl Library for Navidrome {
    fn find_track(&self, track: &Track) -> Result<Option<String>> {
        Navidrome::find_track(self, track)
    }

    fn save_playlist(&mut self, name: &str, song_ids: &[String]) -> Result<()> {
        Navidrome::save_playlist(self, name, song_ids)
    }
}

#[derive(Debug, PartialEq)]
pub struct Synced {
    name: String,
    matched: usize,
    total: usize,
    missing: Vec<Track>,
    skipped: bool,
}

/// Lidarr, the artists it has (lowercase name to id), and the profile ids for new artists.
pub struct LidarrQueue<'a> {
    lidarr: &'a Lidarr,
    artists: &'a mut HashMap<String, i64>,
    quality_id: i64,
    metadata_id: i64,
}

/// Write the tracks as a .spotdl file, leaving out tracks spotdl already tried.
pub fn write_missing_songs<'a>(
    songs: impl IntoIterator<Item = Option<&'a Value>>,
    path: &Path,
    tried: &Path,
) -> Result<usize> {
    let tried_text = if tried.exists() {
        std::fs::read_to_string(tried)?
    } else {
        String::new()
    };
    let skip: HashSet<&str> = tried_text.split_whitespace().collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for song in songs.into_iter().flatten().filter(|s| truthy(s)) {
        let url = song["url"].as_str().unwrap_or("");
        if !url.is_empty() && !skip.contains(url) && seen.insert(url.to_string()) {
            out.push(song);
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string(&out)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(out.len())
}

pub fn sync_playlist(
    playlist: &Value,
    spotify: &mut impl Source,
    navidrome: &mut impl Library,
    lidarr: Option<&mut LidarrQueue>,
    dry_run: bool,
) -> Result<Synced> {
    let name = playlist["name"].as_str().unwrap_or("Untitled").to_string();
    let empty = |skipped| Synced {
        name: name.clone(),
        matched: 0,
        total: 0,
        missing: vec![],
        skipped,
    };
    let id = playlist["id"].as_str().context("playlist without id")?;
    let tracks = match spotify.tracks(id) {
        Ok(tracks) => tracks,
        Err(err) if err.is::<RateLimited>() => {
            eprintln!("  Skipping {}: {err}", py_repr(&name));
            return Ok(empty(true));
        }
        Err(err) => match err.downcast_ref::<HttpStatus>() {
            Some(e) if e.code == 403 => {
                eprintln!("  Skipping {}: 403 {}", py_repr(&name), e.body);
                return Ok(empty(false));
            }
            _ => return Err(err),
        },
    };
    if tracks.is_empty() {
        eprintln!(
            "  {}: no tracks returned from Spotify (empty or unavailable playlist)",
            py_repr(&name)
        );
        return Ok(empty(false));
    }

    // Newest added first, like Spotify's "Date added" sort. The sort is stable, so cached
    // tracks without a date keep Spotify's custom order.
    let mut tracks = tracks;
    tracks.sort_by(|a, b| b.added_at.cmp(&a.added_at));
    let dump = std::env::var_os(DUMP_IDS_ENV).is_some();
    let mut song_ids = Vec::new();
    let mut missing = Vec::new();
    for track in &tracks {
        let found = navidrome.find_track(track)?;
        if dump {
            let shown = found.as_deref().unwrap_or("-");
            eprintln!(
                "ids\t{name}\t{}\t{}\t{shown}",
                track.title,
                track.artists.join(";")
            );
        }
        match found {
            Some(id) => song_ids.push(id),
            None => missing.push(track.clone()),
        }
    }
    let mut seen = HashSet::new();
    song_ids.retain(|id| seen.insert(id.clone()));

    if !dry_run && !song_ids.is_empty() {
        navidrome.save_playlist(&name, &song_ids)?;
    }
    if let Some(queue) = lidarr
        && !dry_run
    {
        queue_missing(queue, &missing);
    }
    Ok(Synced {
        name,
        matched: song_ids.len(),
        total: tracks.len(),
        missing,
        skipped: false,
    })
}

fn queue_missing(queue: &mut LidarrQueue, missing: &[Track]) {
    for track in missing {
        let Some(artist) = track.artists.first() else {
            continue;
        };
        let key = artist.to_lowercase();
        let mut lidarr_id = queue.artists.get(&key).copied();
        if lidarr_id.is_none() {
            let new_id = queue
                .lidarr
                .add_artist(artist, queue.quality_id, queue.metadata_id);
            // 0 marks a failed lookup, so the artist's other tracks do not wait 30 s again.
            queue.artists.insert(key, new_id.unwrap_or(0));
            if let Some(id) = new_id {
                queue.lidarr.refresh_artist(id);
                lidarr_id = Some(id);
            }
        }
        if let Some(id) = lidarr_id.filter(|&id| id != 0)
            && !track.album.is_empty()
            && !queue.lidarr.want_album(id, &track.album)
        {
            eprintln!(
                "  Lidarr: no album match for {} by {}",
                py_repr(&track.album),
                py_repr(artist)
            );
        }
    }
}

/// The value in the env file, else in the environment, as `creds.get(k) or os.environ.get(k)`.
fn setting(file: &HashMap<String, String>, key: &str) -> String {
    file.get(key)
        .filter(|v| !v.is_empty())
        .cloned()
        .or_else(|| std::env::var(key).ok())
        .unwrap_or_default()
}

fn save_env_key(path: &Path, key: &str, value: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let text = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let line = format!("{key}={value}");
    match lines.iter_mut().find(|l| l.starts_with(&format!("{key}="))) {
        Some(old) => *old = line,
        None => lines.push(line),
    }
    let tmp = path.with_extension("tmp");
    // Mode 600 from the start, so the token is never readable by others, not even briefly.
    let _ = std::fs::remove_file(&tmp);
    std::io::Write::write_all(
        &mut std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?,
        (lines.join("\n") + "\n").as_bytes(),
    )?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn cmd_auth(creds_path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let creds = load_env(creds_path);
    let client_id = setting(&creds, "SPOTIFY_CLIENT_ID");
    let client_secret = setting(&creds, "SPOTIFY_CLIENT_SECRET");
    if client_id.is_empty() || client_secret.is_empty() {
        die(&format!(
            "Set SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET in {} first.",
            creds_path.display()
        ));
    }
    let refresh_token = spotify::auth(&client_id, &client_secret)?;
    if let Some(dir) = creds_path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    save_env_key(creds_path, "SPOTIFY_REFRESH_TOKEN", &refresh_token)?;
    println!("Refresh token saved to {}.", creds_path.display());
    Ok(())
}

/// Lidarr when its key is set. A failed first call keeps Lidarr but leaves the rest at the
/// defaults, as the script does.
fn open_lidarr(data: &Path) -> (Option<Lidarr>, HashMap<String, i64>, i64, i64) {
    let key = load_env(&data.join("secrets/lidarr.env"))
        .get("LIDARR__AUTH__APIKEY")
        .cloned()
        .unwrap_or_default();
    if key.is_empty() {
        return (None, HashMap::new(), 1, 1);
    }
    let lidarr = Lidarr::new(&key);
    let (mut artists, mut quality_id, mut metadata_id) = (HashMap::new(), 1, 1);
    let setup = (|| -> Result<()> {
        artists = lidarr.artist_ids_by_name()?;
        quality_id = lidarr.quality_profile_id()?;
        metadata_id = lidarr.metadata_profile_id()?;
        Ok(())
    })();
    if let Err(err) = setup {
        eprintln!("Lidarr not available, skipping: {err}");
    }
    (Some(lidarr), artists, quality_id, metadata_id)
}

pub fn run(args: Args) -> Result<()> {
    let data = data_dir();
    let spotify_creds = data.join("secrets/spotify.env");
    if args.auth {
        return cmd_auth(&spotify_creds);
    }
    let spotify_env = load_env(&spotify_creds);
    let client_id = setting(&spotify_env, "SPOTIFY_CLIENT_ID");
    let client_secret = setting(&spotify_env, "SPOTIFY_CLIENT_SECRET");
    let refresh_token = setting(&spotify_env, "SPOTIFY_REFRESH_TOKEN");
    if client_id.is_empty() || client_secret.is_empty() || refresh_token.is_empty() {
        die(&format!(
            "Run --auth first, or set credentials in {}.",
            spotify_creds.display()
        ));
    }
    let navidrome_creds = data.join("secrets/navidrome.env");
    let password = setting(&load_env(&navidrome_creds), "NAVIDROME_PASSWORD");
    if password.is_empty() {
        die(&format!(
            "Set NAVIDROME_PASSWORD in {} or in the environment.",
            navidrome_creds.display()
        ));
    }
    let client = SpotifyClient::new(&client_id, &client_secret, &refresh_token);
    let mut spotify = CachedSpotify::new(client, &data.join("spotify-cache.json"))?;
    let url = std::env::var("NAVIDROME_URL").unwrap_or_default();
    if url.is_empty() {
        die("Set NAVIDROME_URL in the environment (e.g. https://your-host/music).");
    }
    let user = std::env::var("NAVIDROME_USER").unwrap_or_else(|_| "admin".into());
    let mut navidrome = Navidrome::new(&url, &user, &password, "spotify-sync");
    navidrome.call("ping", &[])?;

    let (lidarr, mut artists, quality_id, metadata_id) = if args.no_lidarr {
        (None, HashMap::new(), 1, 1)
    } else {
        open_lidarr(&data)
    };
    let mut queue = lidarr.as_ref().map(|lidarr| LidarrQueue {
        lidarr,
        artists: &mut artists,
        quality_id,
        metadata_id,
    });

    let mut playlists = spotify.playlists()?;
    playlists.push(json!({"id": LIKED_ID, "name": LIKED_NAME}));
    let mut total_missing = Vec::new();
    let mut skipped = false;
    let prefix = if args.dry_run { "[dry-run] " } else { "" };
    for playlist in &playlists {
        let result = sync_playlist(
            playlist,
            &mut spotify,
            &mut navidrome,
            queue.as_mut(),
            args.dry_run,
        )?;
        println!(
            "{prefix}{}: {}/{} in library, {} missing",
            result.name,
            result.matched,
            result.total,
            result.missing.len()
        );
        total_missing.extend(result.missing);
        skipped |= result.skipped;
    }

    if !total_missing.is_empty() {
        let by_album = most_common(total_missing.iter().map(|t| {
            let artist = t.artists.first().map_or("?", String::as_str);
            (artist.to_string(), t.album.clone())
        }));
        println!(
            "\n{} tracks not in library across {} playlists. Top missing albums:",
            total_missing.len(),
            playlists.len()
        );
        for ((artist, album), count) in by_album.iter().take(10) {
            println!("  {count:3}  {artist} - {album}");
        }
        if lidarr.is_some() && !args.dry_run {
            println!("Missing artists and albums added to Lidarr for download.");
        }
    }
    // A skipped playlist's tracks are unknown; no file keeps the fill loop from ending early.
    if let Some(out) = &args.missing_out
        && !skipped
    {
        let songs = total_missing.iter().map(|t| t.song.as_ref());
        let count = write_missing_songs(songs, out, &data.join("spotdl-tried.txt"))?;
        println!("{count} missing tracks written to {}", out.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spotify::spotdl_song;
    use crate::spotify::tests::{spotify_track, track};

    struct FakeLibrary {
        saved: Option<Vec<String>>,
    }

    impl Library for FakeLibrary {
        fn find_track(&self, track: &Track) -> Result<Option<String>> {
            Ok(Some(track.title.clone()))
        }

        fn save_playlist(&mut self, _name: &str, song_ids: &[String]) -> Result<()> {
            self.saved = Some(song_ids.to_vec());
            Ok(())
        }
    }

    struct ListSpotify(Result<Vec<Track>, &'static str>);

    impl Source for ListSpotify {
        fn playlists(&mut self) -> Result<Vec<Value>> {
            unreachable!()
        }

        fn tracks(&mut self, _id: &str) -> Result<Vec<Track>> {
            match &self.0 {
                Ok(tracks) => Ok(tracks.clone()),
                Err(msg) => Err(RateLimited(msg.to_string()).into()),
            }
        }
    }

    #[test]
    fn rate_limited_playlist_without_cache_is_skipped_not_fatal() {
        let mut spotify = ListSpotify(Err("Spotify rate limit: retry after 9999 s"));
        let mut library = FakeLibrary { saved: None };
        let playlist = json!({"id": "p1", "name": "New"});
        let result = sync_playlist(&playlist, &mut spotify, &mut library, None, true).unwrap();
        let want = Synced {
            name: "New".into(),
            matched: 0,
            total: 0,
            missing: vec![],
            skipped: true,
        };
        assert_eq!(result, want);
    }

    #[test]
    fn playlist_is_saved_newest_added_first_like_spotify_date_added_sort() {
        let dated = |title, at: &str| Track {
            added_at: at.to_string(),
            ..track(title)
        };
        let mut spotify = ListSpotify(Ok(vec![
            dated("old", "2024-01-01T00:00:00Z"),
            dated("new", "2026-10-01T00:00:00Z"),
            dated("mid", "2025-05-01T00:00:00Z"),
        ]));
        let mut library = FakeLibrary { saved: None };
        let playlist = json!({"id": "p1", "name": "P"});
        sync_playlist(&playlist, &mut spotify, &mut library, None, false).unwrap();
        assert_eq!(library.saved.unwrap(), ["new", "mid", "old"]);
    }

    #[test]
    fn missing_songs_leave_out_tried_and_duplicate_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let song = spotdl_song(&spotify_track()).unwrap();
        let mut other = song.clone();
        other["url"] = json!("https://open.spotify.com/track/t2");
        let tried = dir.path().join("tried.txt");
        std::fs::write(&tried, "https://open.spotify.com/track/t2\n").unwrap();
        let out = dir.path().join("missing.spotdl");
        let songs = [Some(&song), Some(&song), Some(&other), None];
        write_missing_songs(songs, &out, &tried).unwrap();
        let written: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        let urls: Vec<&Value> = written.iter().map(|s| &s["url"]).collect();
        assert_eq!(urls, [&json!("https://open.spotify.com/track/t1")]);
    }

    #[test]
    fn env_key_is_replaced_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spotify.env");
        std::fs::write(&path, "SPOTIFY_CLIENT_ID=a\nSPOTIFY_REFRESH_TOKEN=old\n").unwrap();
        save_env_key(&path, "SPOTIFY_REFRESH_TOKEN", "new").unwrap();
        save_env_key(&path, "OTHER", "x").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "SPOTIFY_CLIENT_ID=a\nSPOTIFY_REFRESH_TOKEN=new\nOTHER=x\n"
        );
    }
}

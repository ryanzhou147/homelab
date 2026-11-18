//! Adds a Spotify playlist to the music library: reads its songs from a public playlist link or
//! an Exportify CSV, saves a Navidrome playlist of the songs already in the library, fetches the
//! missing ones from Soulseek one by one, then saves the playlist again with them.
//!
//!     homelab add-playlist https://open.spotify.com/playlist/<id>
//!     homelab add-playlist playlist.csv
//!
//! A link needs no Spotify account: it reads Spotify's public embed page, which lists at most
//! 100 songs and no albums. An Exportify CSV has every song. The web page (playlist_web.rs)
//! runs the same steps.

use std::collections::HashSet;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::common::{data_dir, load_env};
use crate::import::{artists_of, csv_text_rows, field, server_and_user};
use crate::navidrome::{Navidrome, agent};
use crate::soulseek::{self, Wanted};
use crate::spotify::Track;

/// The most songs Spotify's public embed page lists.
pub const LINK_LIMIT: usize = 100;
const SCAN_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(clap::Args)]
pub struct Args {
    /// A public Spotify playlist link, or an Exportify CSV file.
    source: String,
}

pub struct Song {
    pub track: Track,
    pub seconds: u64,
}

pub struct Playlist {
    pub name: String,
    pub songs: Vec<Song>,
    /// Where it came from, kept as the Navidrome playlist's comment: a later add of the same
    /// source replaces it, and a playlist with another comment is never overwritten.
    pub key: String,
    /// True when Spotify's page stopped at LINK_LIMIT songs.
    pub truncated: bool,
}

/// The playlist id of an open.spotify.com playlist link (with or without /intl-xx/ or ?si=).
pub fn playlist_id(link: &str) -> Option<String> {
    let re = regex::Regex::new(r"^https://open\.spotify\.com/(?:intl-[a-z-]+/)?(?:embed/)?playlist/([A-Za-z0-9]{22})(?:[/?#].*)?$").unwrap();
    re.captures(link.trim()).map(|c| c[1].to_string())
}

/// The name and songs in the __NEXT_DATA__ JSON of Spotify's embed page.
/// Artists are joined with ",\u{a0}" (a no-break space), so "Tyler, The Creator" stays whole.
pub fn parse_embed(html: &str) -> Result<Playlist> {
    let start = html
        .find(r#"<script id="__NEXT_DATA__" type="application/json">"#)
        .context("Spotify's page has no playlist data; the link may be private or wrong")?;
    let rest = &html[start..];
    let json = &rest[rest.find('>').unwrap_or(0) + 1..rest.find("</script>").unwrap_or(rest.len())];
    let data: Value = serde_json::from_str(json).context("Spotify's playlist data is not JSON")?;
    let entity = &data["props"]["pageProps"]["state"]["data"]["entity"];
    let name = entity["name"]
        .as_str()
        .context("Spotify's page has no playlist name")?
        .to_string();
    let songs = (entity["trackList"].as_array().into_iter().flatten())
        .filter(|t| t["entityType"] == "track")
        .map(|t| Song {
            track: Track::new(
                t["title"].as_str().unwrap_or_default(),
                t["subtitle"]
                    .as_str()
                    .unwrap_or_default()
                    .split(",\u{a0}")
                    .map(String::from)
                    .collect(),
                "",
            ),
            seconds: t["duration"].as_u64().unwrap_or(0) / 1000,
        })
        .collect();
    Ok(Playlist {
        name,
        songs,
        key: String::new(),
        truncated: false,
    })
}

/// Reads a public playlist link through Spotify's embed page.
pub fn from_link(link: &str) -> Result<Playlist> {
    let id =
        playlist_id(link).context("Not a Spotify playlist link (open.spotify.com/playlist/...)")?;
    let mut resp = agent()
        .get(format!("https://open.spotify.com/embed/playlist/{id}"))
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Safari/537.36")
        .call()
        .context("Spotify did not answer")?;
    if !resp.status().is_success() {
        bail!("Spotify answered HTTP {} for this playlist", resp.status());
    }
    let mut playlist = parse_embed(&resp.body_mut().read_to_string()?)?;
    playlist.key = format!("spotify:playlist:{id}");
    playlist.truncated = playlist.songs.len() >= LINK_LIMIT;
    Ok(playlist)
}

/// Reads an Exportify CSV; the name is the file name without .csv, with '_' as spaces.
pub fn from_csv(file_name: &str, text: &str) -> Result<Playlist> {
    let stem = file_name
        .strip_suffix(".csv")
        .unwrap_or(file_name)
        .replace('_', " ");
    let songs: Vec<Song> = csv_text_rows(text)?
        .iter()
        .filter(|r| !field(r, "Track Name").is_empty())
        .map(|r| Song {
            track: Track::new(
                field(r, "Track Name"),
                artists_of(field(r, "Artist Name(s)")),
                field(r, "Album Name"),
            ),
            seconds: field(r, "Duration (ms)").parse::<u64>().unwrap_or(0) / 1000,
        })
        .collect();
    if songs.is_empty() {
        bail!("No songs in this file; is it an Exportify CSV?");
    }
    let name = stem.trim();
    Ok(Playlist {
        name: if name.is_empty() {
            "Imported playlist".into()
        } else {
            name.into()
        },
        songs,
        key: format!("exportify:{file_name}"),
        truncated: false,
    })
}

fn navidrome() -> Result<Navidrome> {
    let (server, user) = server_and_user()?;
    let env = load_env(&data_dir().join("secrets/navidrome.env"));
    let password = env
        .get("NAVIDROME_PASSWORD")
        .filter(|p| !p.is_empty())
        .context("No NAVIDROME_PASSWORD in DATA/secrets/navidrome.env")?;
    let navidrome = Navidrome::new(&server, &user, password, "add-playlist");
    navidrome.call("ping", &[])?;
    Ok(navidrome)
}

/// The name to save under: the playlist's own unless a playlist of that name came from
/// somewhere else (made by hand, by the Spotify sync, or from another source).
fn target_name(lists: &Value, playlist: &Playlist) -> Result<String> {
    let comment_of = |name: &str| {
        (lists["playlists"]["playlist"]
            .as_array()
            .into_iter()
            .flatten())
        .find(|p| p["name"] == name)
        .map(|p| p["comment"].as_str().unwrap_or("").to_string())
    };
    let candidates = std::iter::once(playlist.name.clone()).chain((1..10).map(|n| match n {
        1 => format!("{} (Spotify)", playlist.name),
        n => format!("{} (Spotify {n})", playlist.name),
    }));
    for name in candidates {
        match comment_of(&name) {
            None => return Ok(name),
            Some(c) if c == playlist.key => return Ok(name),
            Some(_) => {}
        }
    }
    bail!("Too many playlists named \"{}\"", playlist.name)
}

/// Saves the playlist with the songs in the library, marked with its source; returns the
/// songs that are not in the library.
fn save<'a>(
    navidrome: &Navidrome,
    playlist: &'a Playlist,
    name: &str,
    log: &mut dyn FnMut(String),
) -> Result<Vec<&'a Song>> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut missing = Vec::new();
    for song in &playlist.songs {
        match navidrome.find_track(&song.track)? {
            Some(id) => {
                if seen.insert(id.clone()) {
                    ids.push(id);
                }
            }
            None => missing.push(song),
        }
    }
    navidrome.save_playlist(name, &ids)?;
    let id = navidrome
        .playlist_id(name)?
        .context("Navidrome lost the playlist it just saved")?;
    navidrome.call(
        "updatePlaylist",
        &[
            ("playlistId", id.as_str()),
            ("comment", playlist.key.as_str()),
        ],
    )?;
    log(format!(
        "Playlist \"{name}\": {} of {} songs in your library",
        ids.len(),
        playlist.songs.len()
    ));
    Ok(missing)
}

pub fn rescan(navidrome: &Navidrome) -> Result<()> {
    navidrome.call("startScan", &[])?;
    let start = Instant::now();
    while start.elapsed() < SCAN_WAIT {
        sleep(Duration::from_secs(10));
        if navidrome.call("getScanStatus", &[])?["scanStatus"]["scanning"] != true {
            return Ok(());
        }
    }
    bail!("Navidrome is still scanning after 15 min")
}

/// The whole job; `log` gets each step as one line.
pub fn add(playlist: &Playlist, log: &mut dyn FnMut(String)) -> Result<()> {
    log(format!(
        "Read {} songs from \"{}\"",
        playlist.songs.len(),
        playlist.name
    ));
    if playlist.truncated {
        log(format!(
            "Spotify's page shows only the first {LINK_LIMIT} songs; use an Exportify CSV for all of them"
        ));
    }
    let navidrome = navidrome()?;
    let name = target_name(&navidrome.call("getPlaylists", &[])?, playlist)?;
    let missing = save(&navidrome, playlist, &name, log)?;
    if missing.is_empty() {
        log("Done: every song is in your library".into());
        return Ok(());
    }
    log(format!(
        "Looking for {} missing songs on Soulseek",
        missing.len()
    ));
    let wanted: Vec<Wanted> = missing
        .iter()
        .map(|s| Wanted {
            title: &s.track.title,
            artists: &s.track.artists,
            seconds: s.seconds,
        })
        .collect();
    let mut update = |log: &mut dyn FnMut(String)| {
        log("New songs are in; Navidrome is scanning them".into());
        let result = rescan(&navidrome).and_then(|()| save(&navidrome, playlist, &name, log));
        if let Err(e) = &result {
            log(format!("Could not update the playlist yet: {e:#}"));
        }
        result.is_ok()
    };
    let filed = soulseek::fetch(&wanted, log, &mut update)?;
    log(format!(
        "Done: {} of {} missing songs found",
        filed,
        missing.len()
    ));
    Ok(())
}

pub fn run(args: Args) -> Result<()> {
    let playlist = if args.source.starts_with("https://") {
        from_link(&args.source)?
    } else {
        let path = PathBuf::from(&args.source);
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        from_csv(
            &path.file_name().unwrap_or_default().to_string_lossy(),
            &text,
        )?
    };
    add(&playlist, &mut |line| println!("{line}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_playlist_links() {
        let id = "37i9dQZF1DX4UtSsGT1Sbe";
        assert_eq!(
            playlist_id(&format!("https://open.spotify.com/playlist/{id}")).as_deref(),
            Some(id)
        );
        assert_eq!(
            playlist_id(&format!(
                "https://open.spotify.com/intl-de/playlist/{id}?si=abc"
            ))
            .as_deref(),
            Some(id)
        );
        assert_eq!(
            playlist_id(&format!(" https://open.spotify.com/embed/playlist/{id} ")).as_deref(),
            Some(id)
        );
        assert!(playlist_id(&format!("https://evil.example/playlist/{id}")).is_none());
        assert!(playlist_id("https://open.spotify.com/album/37i9dQZF1DX4UtSsGT1Sbe").is_none());
    }

    #[test]
    fn parses_the_embed_page() {
        let html = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{"name":"All Out 80s","trackList":[
            {"title":"Thriller","subtitle":"Michael Jackson","duration":358807,"entityType":"track"},
            {"title":"Under Pressure","subtitle":"Queen,\u00a0David Bowie","duration":248000,"entityType":"track"},
            {"title":"EARFQUAKE","subtitle":"Tyler, The Creator","duration":190000,"entityType":"track"},
            {"title":"An Episode","subtitle":"x","duration":1,"entityType":"episode"}]}}}}}}</script></html>"#;
        let p = parse_embed(html).unwrap();
        assert_eq!(p.name, "All Out 80s");
        assert_eq!(p.songs.len(), 3);
        assert_eq!(p.songs[2].track.artists, ["Tyler, The Creator"]);
        assert_eq!(p.songs[1].track.artists, ["Queen", "David Bowie"]);
        assert_eq!(p.songs[0].seconds, 358);
        assert!(parse_embed("<html>no data</html>").is_err());
    }

    #[test]
    fn reads_exportify_csv() {
        let csv = "\u{feff}Track URI,Track Name,Album Name,Artist Name(s),Duration (ms)\nx,Song A,Album,Artist 1;Artist 2,200000\ny,,Album,Nobody,1\n";
        let p = from_csv("Road_Trip.csv", csv).unwrap();
        assert_eq!(p.name, "Road Trip");
        assert_eq!(p.key, "exportify:Road_Trip.csv");
        assert_eq!(p.songs.len(), 1);
        assert_eq!(p.songs[0].track.artists, ["Artist 1", "Artist 2"]);
        assert_eq!(p.songs[0].seconds, 200);
        assert!(from_csv("x.csv", "a,b\n1,2\n").is_err());
    }

    #[test]
    fn never_takes_over_a_playlist_from_elsewhere() {
        let p = Playlist {
            name: "Gym".into(),
            songs: Vec::new(),
            key: "spotify:playlist:a".into(),
            truncated: false,
        };
        let lists = |entries: Value| json!({"playlists": {"playlist": entries}});
        assert_eq!(target_name(&lists(json!([])), &p).unwrap(), "Gym");
        assert_eq!(
            target_name(
                &lists(json!([{"name": "Gym", "comment": "spotify:playlist:a"}])),
                &p
            )
            .unwrap(),
            "Gym"
        );
        assert_eq!(
            target_name(&lists(json!([{"name": "Gym"}])), &p).unwrap(),
            "Gym (Spotify)"
        );
        let taken = json!([{"name": "Gym", "comment": "mine"}, {"name": "Gym (Spotify)", "comment": "spotify:playlist:b"}]);
        assert_eq!(target_name(&lists(taken), &p).unwrap(), "Gym (Spotify 2)");
    }
}

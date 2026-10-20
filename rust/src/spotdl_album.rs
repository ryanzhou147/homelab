//! Downloads a full album from Spotify/YouTube Music with spotdl:
//!     homelab spotdl-album "Artist" "Album Title"
//! Reads credentials from $DATA/secrets/spotify.env.

use std::ffi::OsString;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::common::{data_dir, home, load_env};

#[derive(clap::Args)]
pub struct Args {
    artist: String,
    album: String,
}

pub fn run(args: Args) -> Result<()> {
    let env = load_env(&data_dir().join("secrets").join("spotify.env"));
    // The Python script also strips quotes around values.
    let get = |k: &str| {
        env.get(k)
            .map_or("", |v| v.trim_matches(['"', '\'']))
            .to_string()
    };
    let (client_id, client_secret, refresh) = (
        get("SPOTIFY_CLIENT_ID"),
        get("SPOTIFY_CLIENT_SECRET"),
        get("SPOTIFY_REFRESH_TOKEN"),
    );
    if client_id.is_empty() || client_secret.is_empty() || refresh.is_empty() {
        eprintln!("Missing Spotify credentials in spotify.env");
        std::process::exit(1);
    }

    let (artist, album) = (&args.artist, &args.album);
    println!("Searching Spotify for: {artist} - {album}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let token = refresh_token(&agent, &client_id, &client_secret, &refresh)?;
    let Some(url) = search_album(&agent, &token, artist, album)? else {
        eprintln!("Not found on Spotify: {artist} - {album}");
        std::process::exit(1);
    };
    println!("Found: {url}");

    // Download to the artist directory with spotdl.
    let artist_dir = home()
        .join("homelab-media")
        .join("music")
        .join(artist_folder(artist)?);
    std::fs::create_dir_all(&artist_dir)?;
    let status = Command::new("spotdl")
        .args(spotdl_args(&url, &client_id, &client_secret))
        .current_dir(&artist_dir)
        .status()
        .context("cannot run spotdl")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn spotdl_args(url: &str, client_id: &str, client_secret: &str) -> Vec<OsString> {
    [
        "download",
        url,
        "--client-id",
        client_id,
        // shortcut: other users on this PC can read the secret in the process list; this PC has
        // one user. Move it to spotdl's config file (mode 600) if that changes.
        "--client-secret",
        client_secret,
        "--output",
        "{album} ({year})/{track-number} - {title}.{output-ext}",
        "--format",
        "mp3",
        "--bitrate",
        "320k",
        "--threads",
        "4",
    ]
    .map(OsString::from)
    .into()
}

fn refresh_token(agent: &ureq::Agent, id: &str, secret: &str, refresh: &str) -> Result<String> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", id),
        ("client_secret", secret),
    ];
    let reply: Value = agent
        .post("https://accounts.spotify.com/api/token")
        .send_form(form)?
        .body_mut()
        .read_json()?;
    reply["access_token"]
        .as_str()
        .map(str::to_string)
        .context("Spotify gave no access_token")
}

fn search_album(
    agent: &ureq::Agent,
    token: &str,
    artist: &str,
    album: &str,
) -> Result<Option<String>> {
    let search = |q: String| -> Result<Vec<Value>> {
        let url = format!(
            "https://api.spotify.com/v1/search?q={}&type=album&limit=5",
            crate::text::quote(&q, "/")
        );
        let data: Value = agent
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .call()?
            .body_mut()
            .read_json()?;
        Ok(data
            .pointer("/albums/items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    };
    let mut items = search(format!("artist:{artist} album:{album}"))?;
    if items.is_empty() {
        // Fallback: looser search.
        items = search(format!("{artist} {album}"))?;
    }
    let Some(best) = items.first() else {
        return Ok(None);
    };
    best.pointer("/external_urls/spotify")
        .and_then(Value::as_str)
        .map(|u| Some(u.to_string()))
        .context("Spotify album has no external_urls.spotify")
}

/// The name comes from Spotify or Lidarr: keep it one folder below music/.
fn artist_folder(artist: &str) -> Result<String> {
    let folder = artist.replace('/', "-");
    anyhow::ensure!(
        !matches!(folder.as_str(), "" | "." | ".."),
        "bad artist folder name {artist:?}"
    );
    Ok(folder)
}

#[cfg(test)]
mod tests {
    #[test]
    fn artist_folder_stays_one_level_below_music() {
        assert_eq!(super::artist_folder("AC/DC").unwrap(), "AC-DC");
        assert!(super::artist_folder("..").is_err() && super::artist_folder("").is_err());
    }

    use super::*;

    #[test]
    fn quote_matches_python() {
        // urllib.parse.quote("artist:A$AP Rocky album:AT.LONG/Этажи 'x'", "/")
        assert_eq!(
            crate::text::quote("artist:A$AP Rocky album:AT.LONG/Этажи 'x'", "/"),
            "artist%3AA%24AP%20Rocky%20album%3AAT.LONG/%D0%AD%D1%82%D0%B0%D0%B6%D0%B8%20%27x%27"
        );
    }

    #[test]
    fn spotdl_command_line_matches_python() {
        let args: Vec<String> = spotdl_args("https://open.spotify.com/album/X", "id", "secret")
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert_eq!(
            args.join(" "),
            "download https://open.spotify.com/album/X --client-id id --client-secret secret \
             --output {album} ({year})/{track-number} - {title}.{output-ext} \
             --format mp3 --bitrate 320k --threads 4"
        );
    }
}

//! The Subsonic API of Navidrome, signed with a salted token so the password never travels.

use crate::spotify::Track;
use crate::text::{MATCH_THRESHOLD, normalize, py_repr, similar, similar_plain, urlencode};
use anyhow::{Result, anyhow, bail};
use md5::{Digest, Md5};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;

const SEARCH_RESULTS: &str = "20";
// createPlaylist sends song ids in the query; 400+ ids pass the ~8 KB nginx URL limit.
const PLAYLIST_CHUNK: usize = 200;
// Tags join artists with " / " (ID3v2.3), "feat.", "•", "&" or ",".
static ARTIST_SEPARATORS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*/\s*|\s+(?:feat\.?|ft\.|featuring|with|x)\s+|\s*[•&,;]\s*").unwrap()
});

/// A blocking HTTP agent with the scripts' 30 s timeout. HTTP errors come back as replies, so
/// callers can read the status, the headers and the body.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// secrets.token_hex(n).
pub fn token_hex(n: usize) -> Result<String> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("random bytes: {e}"))?;
    Ok(hex::encode(bytes))
}

pub struct Navidrome {
    server: String,
    user: String,
    password: String,
    client: &'static str,
    agent: ureq::Agent,
}

impl Navidrome {
    /// `client` is the Subsonic client name: "spotify-sync" or "spotify-import".
    pub fn new(server: &str, user: &str, password: &str, client: &'static str) -> Self {
        Navidrome {
            server: server.trim_end_matches('/').to_string(),
            user: user.to_string(),
            password: password.to_string(),
            client,
            agent: agent(),
        }
    }

    pub fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<Value> {
        let salt = token_hex(8)?;
        let token = hex::encode(Md5::digest(format!("{}{salt}", self.password)));
        let mut query = vec![
            ("u", self.user.as_str()),
            ("t", &token),
            ("s", &salt),
            ("v", "1.16.1"),
            ("c", self.client),
            ("f", "json"),
        ];
        query.extend_from_slice(params);
        let url = format!("{}/rest/{method}?{}", self.server, urlencode(&query));
        let mut resp = self.agent.get(&url).call()?;
        if !resp.status().is_success() {
            bail!("{method}: HTTP Error {}", resp.status().as_u16());
        }
        let body: Value = resp.body_mut().read_json()?;
        let body = body["subsonic-response"].clone();
        if body["status"] != "ok" {
            let msg = body["error"]["message"].as_str().unwrap_or("failed");
            bail!("{method}: {msg}");
        }
        Ok(body)
    }

    fn search(&self, track: &Track) -> Result<Vec<Value>> {
        let artist = track
            .artists
            .first()
            .map(|a| normalize(a))
            .unwrap_or_default();
        let query = format!("{} {artist}", normalize(&track.title));
        let found = self.call(
            "search3",
            &[
                ("query", query.trim()),
                ("songCount", SEARCH_RESULTS),
                ("artistCount", "0"),
                ("albumCount", "0"),
            ],
        )?;
        Ok(found["searchResult3"]["song"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// The sync's match: accent folding, and each part of a multi-artist tag counts.
    pub fn find_track(&self, track: &Track) -> Result<Option<String>> {
        Ok(pick_song(track, &self.search(track)?))
    }

    /// The Exportify import's match: plain similarity, or the artist name inside the tag.
    pub fn find(&self, track: &Track) -> Result<Option<String>> {
        let songs = self.search(track)?;
        Ok(songs.iter().find(|s| is_match(track, s)).map(song_id))
    }

    pub fn playlist_id(&self, name: &str) -> Result<Option<String>> {
        let lists = self.call("getPlaylists", &[])?;
        Ok(lists["playlists"]["playlist"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["name"] == name)
            .map(song_id))
    }

    /// Creates the playlist with the first 200 songs, or replaces the songs of the one with
    /// the same name, then appends the rest in chunks of 200.
    pub fn save_playlist(&self, name: &str, song_ids: &[String]) -> Result<()> {
        let existing = self.playlist_id(name)?;
        let first = &song_ids[..song_ids.len().min(PLAYLIST_CHUNK)];
        let mut params = match &existing {
            Some(id) => vec![("playlistId", id.as_str())],
            None => vec![("name", name)],
        };
        params.extend(first.iter().map(|s| ("songId", s.as_str())));
        let resp = self.call("createPlaylist", &params)?;
        let playlist_id = match (existing, resp["playlist"]["id"].as_str()) {
            (Some(id), _) => id,
            (None, Some(id)) if !id.is_empty() => id.to_string(),
            (None, _) => self.playlist_id(name)?.ok_or_else(|| {
                anyhow!(
                    "Could not create playlist {}: no ID returned",
                    py_repr(name)
                )
            })?,
        };
        for chunk in song_ids[first.len()..].chunks(PLAYLIST_CHUNK) {
            let mut params = vec![("playlistId", playlist_id.as_str())];
            params.extend(chunk.iter().map(|s| ("songIdToAdd", s.as_str())));
            self.call("updatePlaylist", &params)?;
        }
        Ok(())
    }
}

fn song_id(song: &Value) -> String {
    match &song["id"] {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn text<'a>(song: &'a Value, key: &str) -> &'a str {
    song[key].as_str().unwrap_or("")
}

/// The first search result whose title and one artist match the Spotify track.
pub fn pick_song(track: &Track, songs: &[Value]) -> Option<String> {
    songs
        .iter()
        .find(|song| {
            if similar(&track.title, text(song, "title")) < MATCH_THRESHOLD {
                return false;
            }
            let nav_artist = text(song, "artist");
            // The whole tag too, so a name like "Tyler, The Creator" still matches.
            let parts: Vec<&str> = std::iter::once(nav_artist)
                .chain(ARTIST_SEPARATORS.split(nav_artist).map(str::trim))
                .collect();
            track
                .artists
                .iter()
                .any(|a| parts.iter().any(|p| similar(a, p) >= MATCH_THRESHOLD))
        })
        .map(song_id)
}

fn is_match(track: &Track, song: &Value) -> bool {
    if similar_plain(&track.title, text(song, "title")) < MATCH_THRESHOLD {
        return false;
    }
    let song_artist = text(song, "artist");
    let normalized = normalize(song_artist);
    track.artists.iter().any(|a| {
        similar_plain(a, song_artist) >= MATCH_THRESHOLD || normalized.contains(&normalize(a))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn track(title: &str, artists: &[&str]) -> Track {
        Track::new(title, artists.iter().map(|a| a.to_string()).collect(), "")
    }

    fn song(id: &str, title: &str, artist: &str) -> Value {
        json!({"id": id, "title": title, "artist": artist})
    }

    #[test]
    fn find_track_matches_one_part_of_id3_multi_artist_tag() {
        let songs = [song("s1", "STARS", "¥$ / Kanye West / Ty Dolla $ign")];
        let t = track("STARS", &["Kanye West", "Ty Dolla $ign"]);
        assert_eq!(pick_song(&t, &songs).as_deref(), Some("s1"));
    }

    #[test]
    fn find_track_rejects_other_artist_with_same_title() {
        let songs = [song("s1", "Stars", "Grace Potter / The Nocturnals")];
        assert_eq!(pick_song(&track("Stars", &["Kanye West"]), &songs), None);
    }

    #[test]
    fn find_track_matches_feat_and_bullet_artist_tags() {
        let songs = [
            song("s1", "Money Trees", "Kendrick Lamar feat. Jay Rock"),
            song("s2", "Wanna Be Loved", "Joey Bada$$ • JID • Jo-Vaughn"),
        ];
        let t = track("Money Trees", &["Kendrick Lamar", "Jay Rock"]);
        assert_eq!(pick_song(&t, &songs).as_deref(), Some("s1"));
        let t = track("Wanna Be Loved", &["JID"]);
        assert_eq!(pick_song(&t, &songs).as_deref(), Some("s2"));
    }

    #[test]
    fn find_track_keeps_artist_names_that_contain_a_comma() {
        let songs = [song("s1", "EARFQUAKE", "Tyler, The Creator")];
        let t = track("EARFQUAKE", &["Tyler, The Creator"]);
        assert_eq!(pick_song(&t, &songs).as_deref(), Some("s1"));
    }

    #[test]
    fn find_track_matches_slash_tags_without_spaces() {
        let songs = [song("s1", "Flashing Lights", "Kanye West/Dwele")];
        let t = track("Flashing Lights", &["Kanye West", "Dwele"]);
        assert_eq!(pick_song(&t, &songs).as_deref(), Some("s1"));
    }

    #[test]
    fn import_match_accepts_the_artist_inside_a_longer_tag() {
        let t = track("Money Trees", &["Kendrick Lamar"]);
        assert!(is_match(
            &t,
            &song("s1", "Money Trees", "Kendrick Lamar, Jay Rock")
        ));
        assert!(!is_match(&t, &song("s1", "Money Trees", "Jay Rock")));
    }
}

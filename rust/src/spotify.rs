//! The Spotify Web API: the OAuth flow, playlists and Liked Songs, and a cache that keeps the
//! sync working while Spotify rate limits.

use crate::navidrome::{agent, token_hex};
use crate::text::{die, urlencode};
use anyhow::{Context, Result, anyhow};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const AUTH_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const API: &str = "https://api.spotify.com/v1";
const SCOPES: &str = "playlist-read-private playlist-read-collaborative user-library-read";
const REDIRECT_URI: &str = "http://127.0.0.1:18080/callback";
const AUTH_PORT: u16 = 18080;
const AUTH_WAIT: Duration = Duration::from_secs(600);
const RETRIES: usize = 3;
// Spotify can ask for hours after many syncs; fail so the fill loop keeps downloading.
const MAX_RETRY_WAIT: i64 = 300;
/// Liked Songs is not a playlist in the API; this id makes tracks() read /me/tracks instead.
pub const LIKED_ID: &str = "liked";
pub const LIKED_NAME: &str = "Liked Songs";

/// One Spotify track as the sync and the cache keep it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    /// spotdl's Song fields; caches from older versions do not have them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song: Option<Value>,
    #[serde(default)]
    pub added_at: String,
}

impl Track {
    pub fn new(title: &str, artists: Vec<String>, album: &str) -> Self {
        Track {
            title: title.to_string(),
            artists,
            album: album.to_string(),
            song: None,
            added_at: String::new(),
        }
    }
}

#[derive(Debug)]
pub struct RateLimited(pub String);

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RateLimited {}

/// A reply that is not 2xx, with the first 200 characters of its body.
#[derive(Debug)]
pub struct HttpStatus {
    pub code: u16,
    pub body: String,
}

impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "HTTP Error {}: {}", self.code, self.body)
    }
}

impl std::error::Error for HttpStatus {}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python's `a or b`.
fn or<'a>(a: &'a Value, b: &'a Value) -> &'a Value {
    if truthy(a) { a } else { b }
}

/// Python's dict.get(key, default): a key with null keeps the null.
fn get(v: &Value, key: &str, default: Value) -> Value {
    v.get(key).cloned().unwrap_or(default)
}

fn status_error(resp: &mut ureq::http::Response<ureq::Body>) -> HttpStatus {
    let bytes = resp.body_mut().read_to_vec().unwrap_or_default();
    HttpStatus {
        code: resp.status().as_u16(),
        body: String::from_utf8_lossy(&bytes).chars().take(200).collect(),
    }
}

/// GET with the Authorization header. A 429 waits for Retry-After and tries again, up to three
/// times, but fails with RateLimited at once when the wait is longer than 300 s.
pub fn http_get(agent: &ureq::Agent, url: &str, auth: &str) -> Result<Value> {
    for attempt in 0..RETRIES {
        let mut resp = agent.get(url).header("Authorization", auth).call()?;
        if resp.status().as_u16() == 429 {
            let wait = match resp.headers().get("Retry-After") {
                None => 60,
                // An HTTP date, not seconds, counts as too long.
                Some(v) => v
                    .to_str()
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(MAX_RETRY_WAIT + 1),
            };
            if wait > MAX_RETRY_WAIT || attempt == RETRIES - 1 {
                return Err(
                    RateLimited(format!("Spotify rate limit: retry after {wait} s")).into(),
                );
            }
            std::thread::sleep(Duration::from_secs(wait.max(0) as u64));
            continue;
        }
        if !resp.status().is_success() {
            return Err(status_error(&mut resp).into());
        }
        return Ok(resp.body_mut().read_json()?);
    }
    unreachable!("the last attempt returns")
}

/// POST a form, as the token endpoint wants.
fn http_post(agent: &ureq::Agent, url: &str, form: &[(&str, &str)], auth: &str) -> Result<Value> {
    let mut resp = agent
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Authorization", auth)
        .send(urlencode(form))?;
    if !resp.status().is_success() {
        return Err(status_error(&mut resp).into());
    }
    Ok(resp.body_mut().read_json()?)
}

fn basic_auth(id: &str, secret: &str) -> String {
    let creds = base64::engine::general_purpose::STANDARD.encode(format!("{id}:{secret}"));
    format!("Basic {creds}")
}

/// spotdl's Song fields, from the playlist reply, so spotdl downloads with no Spotify calls.
pub fn spotdl_song(track: &Value) -> Result<Value> {
    let empty = json!({});
    let album = or(&track["album"], &empty);
    let artists: Vec<Value> = track["artists"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| a["name"].clone())
        .collect();
    let first_artist = artists.first().cloned().unwrap_or(json!(""));
    let date = or(&album["release_date"], &json!("0")).clone();
    let year: String = date.as_str().unwrap_or("0").chars().take(4).collect();
    let year: i64 = year
        .trim()
        .parse()
        .with_context(|| format!("release date {date}"))?;
    let images = or(&album["images"], &json!([])).clone();
    let album_artist = match album["artists"].as_array().and_then(|a| a.first()) {
        Some(a) if truthy(&album["artists"]) => a["name"].clone(),
        _ => first_artist.clone(),
    };
    let duration_ms = or(&track["duration_ms"], &json!(0)).as_f64().unwrap_or(0.0);
    let artist_id = match track["artists"].as_array().and_then(|a| a.first()) {
        Some(a) if truthy(&track["artists"]) => a["id"].clone(),
        _ => Value::Null,
    };
    Ok(json!({
        "name": get(track, "name", json!("")), "artists": artists, "artist": first_artist,
        "genres": [], "disc_number": get(track, "disc_number", json!(1)), "disc_count": 1,
        "album_name": get(album, "name", json!("")), "album_artist": album_artist,
        // Python's round() rounds half to even.
        "duration": (duration_ms / 1000.0).round_ties_even() as i64, "year": year, "date": date,
        "track_number": get(track, "track_number", json!(1)),
        "tracks_count": get(album, "total_tracks", json!(1)),
        "song_id": get(track, "id", json!("")), "explicit": get(track, "explicit", json!(false)),
        "publisher": "", "url": get(or(&track["external_urls"], &empty), "spotify", json!("")),
        "isrc": or(&track["external_ids"], &empty)["isrc"].clone(),
        "cover_url": images.get(0).map_or(Value::Null, |i| i["url"].clone()),
        "copyright_text": null, "popularity": track["popularity"].clone(),
        "album_id": album["id"].clone(), "artist_id": artist_id,
        "album_type": album["album_type"].clone(),
    }))
}

/// Where the sync reads playlists and tracks: Spotify itself or the cache in front of it.
pub trait Source {
    fn playlists(&mut self) -> Result<Vec<Value>>;
    fn tracks(&mut self, playlist_id: &str) -> Result<Vec<Track>>;
}

pub struct SpotifyClient {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    access_token: String,
    expires_at: Option<Instant>,
    api: String,
    agent: ureq::Agent,
}

impl SpotifyClient {
    pub fn new(client_id: &str, client_secret: &str, refresh_token: &str) -> Self {
        SpotifyClient {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            refresh_token: refresh_token.to_string(),
            access_token: String::new(),
            expires_at: None,
            api: API.to_string(),
            agent: agent(),
        }
    }

    fn ensure_token(&mut self) -> Result<()> {
        let now = Instant::now();
        if self
            .expires_at
            .is_some_and(|e| now + Duration::from_secs(30) < e)
        {
            return Ok(());
        }
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", self.refresh_token.as_str()),
        ];
        let auth = basic_auth(&self.client_id, &self.client_secret);
        let resp = http_post(&self.agent, TOKEN_URL, &form, &auth)?;
        self.access_token = resp["access_token"]
            .as_str()
            .context("no access_token in the token reply")?
            .to_string();
        let expires_in = resp["expires_in"]
            .as_f64()
            .context("no expires_in in the token reply")?;
        self.expires_at = Some(now + Duration::from_secs_f64(expires_in));
        Ok(())
    }

    fn get(&mut self, url: &str) -> Result<Value> {
        self.ensure_token()?;
        http_get(&self.agent, url, &format!("Bearer {}", self.access_token))
    }

    fn next_url(page: &Value) -> Option<String> {
        page["next"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
    }
}

impl Source for SpotifyClient {
    /// All playlists the current user owns or follows.
    fn playlists(&mut self) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut url = Some(format!("{}/me/playlists?limit=50", self.api));
        while let Some(u) = url {
            let data = self.get(&u)?;
            items.extend(data["items"].as_array().cloned().unwrap_or_default());
            url = Self::next_url(&data);
        }
        Ok(items)
    }

    /// GET /playlists/{id} pages through its top-level 'items' object, because the /tracks
    /// sub-endpoint is blocked in development mode. Liked Songs reads /me/tracks, 50 per page.
    fn tracks(&mut self, playlist_id: &str) -> Result<Vec<Track>> {
        let liked = playlist_id == LIKED_ID;
        let mut url = Some(if liked {
            format!("{}/me/tracks?limit=50", self.api)
        } else {
            format!("{}/playlists/{playlist_id}?limit=100", self.api)
        });
        let mut first = !liked;
        let mut result = Vec::new();
        let empty = json!({});
        while let Some(u) = url {
            let data = self.get(&u)?;
            let page = if first {
                or(or(&data["items"], &data["tracks"]), &empty).clone()
            } else {
                data
            };
            first = false;
            for item in page["items"].as_array().into_iter().flatten() {
                let track = or(&item["item"], &item["track"]);
                if !truthy(track) || truthy(&item["is_local"]) || truthy(&track["is_local"]) {
                    continue;
                }
                if !matches!(&track["type"], Value::Null) && track["type"] != "track" {
                    continue;
                }
                result.push(Track {
                    title: track["name"].as_str().unwrap_or("").to_string(),
                    artists: track["artists"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|a| a["name"].as_str().unwrap_or("").to_string())
                        .collect(),
                    album: track["album"]["name"].as_str().unwrap_or("").to_string(),
                    song: Some(spotdl_song(track)?),
                    added_at: item["added_at"].as_str().unwrap_or("").to_string(),
                });
            }
            url = Self::next_url(&page);
        }
        Ok(result)
    }
}

/// Fetches a playlist's tracks only when its snapshot_id changes, and serves the last copy
/// while Spotify rate limits. Each fill-loop round then costs one API call, plus about 15 for
/// Liked Songs, which has no snapshot_id.
pub struct CachedSpotify<S> {
    client: S,
    path: PathBuf,
    cache: Value,
    limited: bool,
}

impl<S: Source> CachedSpotify<S> {
    pub fn new(client: S, path: &Path) -> Result<Self> {
        let cache = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).ok().filter(Value::is_object),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        Ok(CachedSpotify {
            client,
            path: path.to_path_buf(),
            cache: cache.unwrap_or_else(|| json!({"playlists": [], "tracks": {}})),
            limited: false,
        })
    }

    fn save(&self) -> Result<()> {
        let tmp = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string(&self.cache)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn cached_tracks(entry: &Value) -> Result<Vec<Track>> {
    Ok(serde_json::from_value(entry["tracks"].clone())?)
}

impl<S: Source> Source for CachedSpotify<S> {
    fn playlists(&mut self) -> Result<Vec<Value>> {
        match self.client.playlists() {
            Ok(fresh) => {
                self.cache["playlists"] = Value::Array(fresh.clone());
                self.save()?;
                Ok(fresh)
            }
            Err(err) if err.is::<RateLimited>() => {
                let cached = self.cache["playlists"].as_array().cloned();
                match cached.filter(|c| !c.is_empty()) {
                    None => Err(err),
                    Some(cached) => {
                        eprintln!("{err}; using cached playlists");
                        self.limited = true;
                        Ok(cached)
                    }
                }
            }
            Err(err) => Err(err),
        }
    }

    fn tracks(&mut self, playlist_id: &str) -> Result<Vec<Track>> {
        let snapshot = self.cache["playlists"]
            .as_array()
            .and_then(|ps| ps.iter().find(|p| p["id"] == playlist_id))
            .map_or(Value::Null, |p| p["snapshot_id"].clone());
        let entry = self.cache["tracks"]
            .get(playlist_id)
            .filter(|e| truthy(e))
            .cloned();
        if let Some(entry) = &entry
            && (self.limited || (truthy(&snapshot) && entry["snapshot"] == snapshot))
        {
            return cached_tracks(entry);
        }
        match self.client.tracks(playlist_id) {
            Ok(fresh) => {
                self.cache["tracks"][playlist_id] = json!({"snapshot": snapshot, "tracks": fresh});
                self.save()?;
                Ok(fresh)
            }
            Err(err) if err.is::<RateLimited>() => match entry {
                None => Err(err),
                Some(entry) => {
                    self.limited = true;
                    cached_tracks(&entry)
                }
            },
            Err(err) => Err(err),
        }
    }
}

/// urllib.parse.unquote_plus.
fn unquote_plus(s: &str) -> String {
    let s = s.replace('+', " ");
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) if bytes[i] == b'%' => {
                out.push(b);
                i += 3;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The first value of each query parameter, as parse_qs(...)[key][0] gives it.
fn query_params(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    let mut params = HashMap::new();
    for (k, v) in query.split('&').filter_map(|kv| kv.split_once('=')) {
        if !v.is_empty() {
            params.entry(unquote_plus(k)).or_insert(unquote_plus(v));
        }
    }
    params
}

/// The OAuth flow: prints the consent URL, waits for the callback and returns the refresh token.
pub fn auth(client_id: &str, client_secret: &str) -> Result<String> {
    let state = token_hex(8)?;
    let params = urlencode(&[
        ("client_id", client_id),
        ("response_type", "code"),
        ("redirect_uri", REDIRECT_URI),
        ("scope", SCOPES),
        ("state", &state),
    ]);
    println!("\nOpen this URL in your browser:\n\n  {AUTH_URL}?{params}\n");
    let server = tiny_http::Server::http(("127.0.0.1", AUTH_PORT)).map_err(|e| anyhow!(e))?;
    println!("Waiting for authorization on port {AUTH_PORT}...");
    let deadline = Instant::now() + AUTH_WAIT;
    let mut code = None;
    while code.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        let Some(request) = server.recv_timeout(left)? else {
            break;
        };
        let q = query_params(request.url());
        if q.get("state") != Some(&state) {
            let reply = tiny_http::Response::from_string("State mismatch").with_status_code(400);
            let _ = request.respond(reply);
            continue;
        }
        code = Some(q.get("code").cloned().unwrap_or_default());
        let header = tiny_http::Header::from_bytes("Content-Type", "text/plain").unwrap();
        let reply =
            tiny_http::Response::from_string("Authorization received. You can close this tab.")
                .with_header(header);
        let _ = request.respond(reply);
    }
    drop(server);
    let Some(code) = code else {
        die("No authorization code received within 10 minutes.")
    };
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", REDIRECT_URI),
    ];
    let resp = http_post(
        &agent(),
        TOKEN_URL,
        &form,
        &basic_auth(client_id, client_secret),
    )?;
    Ok(resp["refresh_token"]
        .as_str()
        .context("no refresh_token in the token reply")?
        .to_string())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    pub fn spotify_track() -> Value {
        json!({
            "id": "t1", "name": "PRIDE.", "duration_ms": 275000, "explicit": true, "disc_number": 1,
            "track_number": 6, "popularity": 70, "external_ids": {"isrc": "USUM71703083"},
            "external_urls": {"spotify": "https://open.spotify.com/track/t1"},
            "artists": [{"name": "Kendrick Lamar", "id": "a1"}],
            "album": {"id": "al1", "name": "DAMN.", "release_date": "2017-04-14", "total_tracks": 14,
                      "album_type": "album", "artists": [{"name": "Kendrick Lamar"}],
                      "images": [{"url": "https://i.scdn.co/image/x"}]},
        })
    }

    const SPOTDL_REQUIRED: &[&str] = &[
        "name",
        "artists",
        "artist",
        "genres",
        "disc_number",
        "disc_count",
        "album_name",
        "album_artist",
        "duration",
        "year",
        "date",
        "track_number",
        "tracks_count",
        "song_id",
        "explicit",
        "publisher",
        "url",
        "isrc",
        "cover_url",
        "copyright_text",
    ];

    /// A status, headers and body.
    type Reply = (u16, Vec<(&'static str, &'static str)>, String);

    /// A local server that gives each request the next reply and records the URLs.
    fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let urls = Arc::new(Mutex::new(Vec::new()));
        let seen = urls.clone();
        std::thread::spawn(move || {
            for (code, headers, body) in replies {
                let request = server.recv().unwrap();
                seen.lock().unwrap().push(request.url().to_string());
                let mut reply = tiny_http::Response::from_string(body).with_status_code(code);
                for (k, v) in headers {
                    reply.add_header(tiny_http::Header::from_bytes(k, v).unwrap());
                }
                request.respond(reply).unwrap();
            }
        });
        (base, urls)
    }

    fn rate_limit_fails_fast(retry_after: &'static str) {
        let (base, _) = serve(vec![(
            429,
            vec![("Retry-After", retry_after)],
            String::new(),
        )]);
        let start = Instant::now();
        let err = http_get(&agent(), &format!("{base}/v1/me"), "Bearer x").unwrap_err();
        assert!(err.is::<RateLimited>() && err.to_string().contains("rate limit"));
        assert!(start.elapsed() < Duration::from_secs(5), "slept");
    }

    #[test]
    fn long_spotify_rate_limit_fails_fast_instead_of_sleeping() {
        rate_limit_fails_fast("7200");
    }

    #[test]
    fn rate_limit_with_http_date_fails_fast() {
        rate_limit_fails_fast("Fri, 16 Oct 2026 23:00:00 GMT");
    }

    #[test]
    fn liked_songs_are_read_from_the_saved_tracks_endpoint() {
        let mut track = spotify_track();
        track["type"] = json!("track");
        let page =
            json!({"items": [{"added_at": "2026-10-01T00:00:00Z", "track": track}], "next": null});
        let (base, urls) = serve(vec![(200, vec![], page.to_string())]);
        let mut client = SpotifyClient::new("id", "secret", "refresh");
        client.api = base;
        client.access_token = "x".into();
        client.expires_at = Some(Instant::now() + Duration::from_secs(3600));
        let tracks = client.tracks(LIKED_ID).unwrap();
        assert_eq!(*urls.lock().unwrap(), ["/me/tracks?limit=50"]);
        assert_eq!(tracks[0].added_at, "2026-10-01T00:00:00Z");
    }

    #[test]
    fn spotdl_song_has_every_field_spotdl_needs() {
        let song = spotdl_song(&spotify_track()).unwrap();
        for key in SPOTDL_REQUIRED {
            assert!(song.get(key).is_some(), "{key}");
        }
        assert_eq!(
            (
                &song["name"],
                &song["album_name"],
                &song["year"],
                &song["duration"]
            ),
            (&json!("PRIDE."), &json!("DAMN."), &json!(2017), &json!(275))
        );
        assert_eq!(song["url"], "https://open.spotify.com/track/t1");
    }

    #[test]
    fn spotdl_song_accepts_a_year_only_release_date() {
        let mut track = spotify_track();
        track["album"]["release_date"] = json!("1998");
        assert_eq!(spotdl_song(&track).unwrap()["year"], 1998);
    }

    #[test]
    fn spotdl_song_accepts_a_null_duration() {
        let mut track = spotify_track();
        track["duration_ms"] = Value::Null;
        assert_eq!(spotdl_song(&track).unwrap()["duration"], 0);
    }

    #[test]
    fn spotdl_song_rounds_half_to_even_like_python() {
        let mut track = spotify_track();
        track["duration_ms"] = json!(2500);
        assert_eq!(spotdl_song(&track).unwrap()["duration"], 2);
    }

    pub struct FakeSpotify {
        pub playlists: Vec<Value>,
        pub tracks: HashMap<String, Vec<Track>>,
        pub calls: Vec<String>,
        pub limited: bool,
    }

    impl Source for &mut FakeSpotify {
        fn playlists(&mut self) -> Result<Vec<Value>> {
            if self.limited {
                return Err(RateLimited("Spotify rate limit: retry after 9999 s".into()).into());
            }
            Ok(self.playlists.clone())
        }

        fn tracks(&mut self, playlist_id: &str) -> Result<Vec<Track>> {
            if self.limited {
                return Err(RateLimited("Spotify rate limit: retry after 9999 s".into()).into());
            }
            self.calls.push(playlist_id.to_string());
            Ok(self.tracks[playlist_id].clone())
        }
    }

    pub fn track(title: &str) -> Track {
        Track::new(title, vec!["A".into()], "B")
    }

    fn fake(snapshot: &str, title: &str) -> FakeSpotify {
        FakeSpotify {
            playlists: vec![json!({"id": "p1", "name": "P", "snapshot_id": snapshot})],
            tracks: HashMap::from([("p1".to_string(), vec![track(title)])]),
            calls: vec![],
            limited: false,
        }
    }

    #[test]
    fn cache_skips_track_fetch_when_snapshot_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let mut f = fake("s1", "x");
        let mut first = CachedSpotify::new(&mut f, &path).unwrap();
        first.playlists().unwrap();
        assert_eq!(first.tracks("p1").unwrap(), [track("x")]);
        let mut second = CachedSpotify::new(&mut f, &path).unwrap();
        second.playlists().unwrap();
        assert_eq!(second.tracks("p1").unwrap(), [track("x")]);
        assert_eq!(f.calls, ["p1"]);
    }

    #[test]
    fn cache_fetches_again_when_snapshot_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let mut f = fake("s1", "x");
        let mut c = CachedSpotify::new(&mut f, &path).unwrap();
        c.playlists().unwrap();
        c.tracks("p1").unwrap();
        let mut f = fake("s2", "y");
        let mut c2 = CachedSpotify::new(&mut f, &path).unwrap();
        c2.playlists().unwrap();
        assert_eq!(c2.tracks("p1").unwrap(), [track("y")]);
    }

    #[test]
    fn cache_is_used_while_spotify_rate_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let mut f = fake("s1", "x");
        let mut c = CachedSpotify::new(&mut f, &path).unwrap();
        c.playlists().unwrap();
        c.tracks("p1").unwrap();
        f.limited = true;
        let mut c2 = CachedSpotify::new(&mut f, &path).unwrap();
        let ids: Vec<Value> = c2
            .playlists()
            .unwrap()
            .iter()
            .map(|p| p["id"].clone())
            .collect();
        assert_eq!(ids, [json!("p1")]);
        assert_eq!(c2.tracks("p1").unwrap(), [track("x")]);
    }

    #[test]
    fn rate_limit_without_cache_still_raises() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = fake("s1", "x");
        f.playlists.clear();
        f.limited = true;
        let mut c = CachedSpotify::new(&mut f, &dir.path().join("cache.json")).unwrap();
        assert!(c.playlists().unwrap_err().is::<RateLimited>());
    }

    #[test]
    fn auth_callback_query_is_decoded() {
        let q = query_params("/callback?code=AQ%2Bx+y&state=ab12&state=zz&empty=");
        assert_eq!(q["code"], "AQ+x y");
        assert_eq!(q["state"], "ab12");
        assert!(!q.contains_key("empty"));
    }
}

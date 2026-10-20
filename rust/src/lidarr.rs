//! The Lidarr API: add the artists of missing tracks and mark their albums as wanted.

use crate::navidrome::agent;
use crate::spotify::truthy;
use crate::text::{normalize, py_repr, quote, similar};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;

const LIDARR_URL: &str = "http://127.0.0.1:8686/lidarr/api/v1";
const ROOT_FOLDER: &str = "/data/music";
const ALBUM_MATCH: f64 = 0.7;

pub struct Lidarr {
    key: String,
    url: String,
    agent: ureq::Agent,
}

impl Lidarr {
    pub fn new(api_key: &str) -> Self {
        Lidarr {
            key: api_key.to_string(),
            url: LIDARR_URL.to_string(),
            agent: agent(),
        }
    }

    /// Every failure, also a timeout, is an error with the method and path, as in the script.
    fn call(&self, path: &str, method: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}/{path}", self.url);
        let fail = |e: &dyn std::fmt::Display| anyhow!("{method} {path}: {e}");
        let body = body.map(|b| b.to_string());
        let resp = match (method, body) {
            ("GET", _) => self.request(self.agent.get(&url)).call(),
            ("POST", Some(b)) => self.request(self.agent.post(&url)).send(b),
            ("PUT", Some(b)) => self.request(self.agent.put(&url)).send(b),
            _ => return Err(fail(&"unsupported method")),
        };
        let mut resp = resp.map_err(|e| fail(&e))?;
        let bytes = resp.body_mut().read_to_vec().map_err(|e| fail(&e))?;
        if !resp.status().is_success() {
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]);
            let code = resp.status().as_u16();
            return Err(fail(&format!("{code} b{}", py_repr(&head))));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| fail(&e))
    }

    fn request<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        req.header("Content-Type", "application/json")
            .header("X-Api-Key", &self.key)
    }

    fn first_id(&self, path: &str) -> Result<i64> {
        let profiles = self.call(path, "GET", None)?;
        Ok(profiles
            .as_array()
            .and_then(|p| p.first())
            .and_then(|p| p["id"].as_i64())
            .unwrap_or(1))
    }

    pub fn artist_ids_by_name(&self) -> Result<HashMap<String, i64>> {
        let artists = self.call("artist", "GET", None)?;
        Ok(artists
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| Some((a["artistName"].as_str()?.to_lowercase(), a["id"].as_i64()?)))
            .collect())
    }

    pub fn quality_profile_id(&self) -> Result<i64> {
        self.first_id("qualityprofile")
    }

    pub fn metadata_profile_id(&self) -> Result<i64> {
        self.first_id("metadataprofile")
    }

    pub fn add_artist(&self, name: &str, quality_id: i64, metadata_id: i64) -> Option<i64> {
        let added = (|| -> Result<Option<i64>> {
            let results = self.call(
                &format!("artist/lookup?term={}", quote(name, "/")),
                "GET",
                None,
            )?;
            let Some(artist) = results.as_array().and_then(|r| r.first()) else {
                return Ok(None);
            };
            let added = self.call(
                "artist",
                "POST",
                Some(json!({
                    "foreignArtistId": artist["foreignArtistId"],
                    "artistName": artist["artistName"],
                    "qualityProfileId": quality_id,
                    "metadataProfileId": metadata_id,
                    "monitored": true,
                    "monitorNewItems": "none",
                    "rootFolderPath": ROOT_FOLDER,
                    "addOptions": {"monitor": "future", "searchForMissingAlbums": false},
                })),
            )?;
            Ok(added["id"].as_i64())
        })();
        added.unwrap_or_else(|err| {
            let msg = err.to_string().to_lowercase();
            let known = ["already exists", "duplicate", "already been added"];
            if !known.iter().any(|k| msg.contains(k)) {
                eprintln!("Warning: could not add {} to Lidarr: {err}", py_repr(name));
            }
            None
        })
    }

    /// Lidarr fetches the artist's discography in the background.
    pub fn refresh_artist(&self, artist_id: i64) {
        let body = json!({"name": "RefreshArtist", "artistId": artist_id});
        if let Err(err) = self.call("command", "POST", Some(body)) {
            eprintln!("Warning: artist refresh failed: {err}");
        }
    }

    /// Marks the closest album of the artist as monitored and starts a search for it.
    pub fn want_album(&self, artist_id: i64, album_name: &str) -> bool {
        self.try_want_album(artist_id, album_name)
            .unwrap_or_else(|err| {
                eprintln!(
                    "Warning: could not mark album {} as wanted: {err}",
                    py_repr(album_name)
                );
                false
            })
    }

    fn try_want_album(&self, artist_id: i64, album_name: &str) -> Result<bool> {
        let albums = self.call(&format!("album?artistId={artist_id}"), "GET", None)?;
        let albums = albums.as_array().cloned().unwrap_or_default();
        // An exact normalized title wins over a stem boost, so "Fearless" does not lose to
        // "Fearless (Fan Edition)" by list order. The first best one wins, as in max().
        let wanted = normalize(album_name);
        let rank = |a: &Value| {
            let t = a["title"].as_str().unwrap_or("");
            (wanted == normalize(t), similar(album_name, t))
        };
        let Some(mut best) = albums.first() else {
            return Ok(false);
        };
        for album in &albums[1..] {
            if rank(album) > rank(best) {
                best = album;
            }
        }
        if similar(album_name, best["title"].as_str().unwrap_or("")) < ALBUM_MATCH {
            return Ok(false);
        }
        if truthy(&best["monitored"]) {
            return Ok(true);
        }
        let mut body = json!({
            "id": best["id"], "foreignAlbumId": best["foreignAlbumId"], "monitored": true,
        });
        if !best["qualityProfileId"].is_null() {
            body["qualityProfileId"] = best["qualityProfileId"].clone();
        }
        self.call(&format!("album/{}", best["id"]), "PUT", Some(body))?;
        let search = json!({"name": "AlbumSearch", "albumIds": [best["id"]]});
        self.call("command", "POST", Some(search))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lidarr_timeout_skips_the_artist_instead_of_stopping_the_sync() {
        // A closed port fails like a timeout: an error the sync passes over.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut lidarr = Lidarr::new("key");
        lidarr.url = format!("http://127.0.0.1:{port}/lidarr/api/v1");
        assert_eq!(lidarr.add_artist("Kendrick Lamar", 1, 1), None);
    }
}

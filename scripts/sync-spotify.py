#!/usr/bin/env python3
"""Syncs Spotify playlists to Navidrome and queues missing albums in Lidarr.

One-time OAuth setup (creates $DATA/secrets/spotify.env):
    ./scripts/sync-spotify.py --auth

Daily sync (the timer runs this automatically):
    ./scripts/sync-spotify.py [--dry-run] [--no-lidarr]

Setup:
  1. Go to https://developer.spotify.com/dashboard and create an app.
  2. Add http://localhost:18080/callback as a Redirect URI.
  3. Create $DATA/secrets/spotify.env with:
         SPOTIFY_CLIENT_ID=<id>
         SPOTIFY_CLIENT_SECRET=<secret>
  4. Run --auth once. It opens a browser URL, waits for the callback, and saves
     SPOTIFY_REFRESH_TOKEN into the same file.
  5. Add NAVIDROME_PASSWORD to $DATA/secrets/navidrome.env (or set it in the environment).
  6. Run start-music-fetch.sh to install the daily timer.
"""

import argparse
import base64
import hashlib
import http.server
import json
import os
import re
import secrets
import sys
import threading
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict
from difflib import SequenceMatcher
from pathlib import Path

DATA = Path(os.environ.get("DATA", Path.home() / "homelab-data"))
SPOTIFY_CREDS = DATA / "secrets/spotify.env"
NAVIDROME_CREDS = DATA / "secrets/navidrome.env"
LIDARR_CREDS = DATA / "secrets/lidarr.env"
SPOTIFY_CACHE = DATA / "spotify-cache.json"
SPOTDL_TRIED = DATA / "spotdl-tried.txt"

NAVIDROME_URL = os.environ.get("NAVIDROME_URL", "")
NAVIDROME_USER = os.environ.get("NAVIDROME_USER", "admin")
LIDARR_URL = "http://127.0.0.1:8686/lidarr/api/v1"

SPOTIFY_AUTH_URL = "https://accounts.spotify.com/authorize"
SPOTIFY_TOKEN_URL = "https://accounts.spotify.com/api/token"
SPOTIFY_API = "https://api.spotify.com/v1"
SCOPES = "playlist-read-private playlist-read-collaborative"
REDIRECT_URI = "http://127.0.0.1:18080/callback"
AUTH_PORT = 18080

MATCH_THRESHOLD = 0.85
SEARCH_RESULTS = 20
# Spotify can ask for hours after many syncs; fail so the fill loop keeps downloading.
MAX_RETRY_WAIT = 300
ARTIST_SEPARATORS = re.compile(r"\s*/\s*|\s+(?:feat\.?|ft\.|featuring|with|x)\s+|\s*[•&,;]\s*", re.I)
ROOT_FOLDER = "/data/music"

NOISE = re.compile(
    r"\s*[\(\[-][^)\]]*\b(remaster(ed)?|feat\.?|ft\.?|with|version|edit|mono|stereo|live|"
    r"deluxe|bonus|explicit)\b[^)\]]*[\)\]]?",
    re.IGNORECASE,
)
# Strips ALL parenthetical annotations (used for stem comparison only).
# Only parens and brackets; dash-prefixed annotations are handled by NOISE.
_ALL_PARENS = re.compile(r"\s*[\(\[][^\)\]]*[\)\]]?")


# --- helpers -----------------------------------------------------------------

def load_env(path: Path) -> dict[str, str]:
    result = {}
    if not path.exists():
        return result
    for line in path.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            result[k.strip()] = v.strip()
    return result


def save_env_key(path: Path, key: str, value: str) -> None:
    lines = path.read_text().splitlines() if path.exists() else []
    updated = False
    for i, line in enumerate(lines):
        if line.startswith(f"{key}="):
            lines[i] = f"{key}={value}"
            updated = True
            break
    if not updated:
        lines.append(f"{key}={value}")
    tmp = path.with_suffix(".tmp")
    tmp.write_text("\n".join(lines) + "\n")
    tmp.chmod(0o600)
    tmp.rename(path)


def normalize(text: str) -> str:
    text = NOISE.sub("", text or "").lower()
    text = re.sub(r"[^\w\s]", " ", text)
    return " ".join(text.split())


def _fold_accents(text: str) -> str:
    return "".join(c for c in unicodedata.normalize("NFKD", text or "")
                   if not unicodedata.combining(c))


def similar(a: str, b: str) -> float:
    a, b = _fold_accents(a), _fold_accents(b)
    na, nb = normalize(a), normalize(b)
    base = SequenceMatcher(None, na, nb).ratio()
    # Parenthetical annotations like "(feat. Drake)", "(PMEDIA)", "(Deluxe)" on one
    # side but not the other lower the ratio without indicating a real mismatch.
    # Compare bare stems (all parens stripped) and boost when they are equal.
    sa = normalize(_ALL_PARENS.sub("", a or ""))
    sb = normalize(_ALL_PARENS.sub("", b or ""))
    if sa and sb and sa == sb:
        return max(base, 0.95)
    return base


def spotdl_song(track: dict) -> dict:
    """spotdl's Song fields, from the playlist reply, so spotdl downloads with no Spotify calls."""
    album = track.get("album") or {}
    artists = [a["name"] for a in track.get("artists", [])]
    date = album.get("release_date") or "0"
    images = album.get("images") or []
    return {
        "name": track.get("name", ""), "artists": artists, "artist": artists[0] if artists else "",
        "genres": [], "disc_number": track.get("disc_number", 1), "disc_count": 1,
        "album_name": album.get("name", ""),
        "album_artist": (album.get("artists") or [{"name": artists[0] if artists else ""}])[0]["name"],
        "duration": round((track.get("duration_ms") or 0) / 1000), "year": int(date[:4]), "date": date,
        "track_number": track.get("track_number", 1), "tracks_count": album.get("total_tracks", 1),
        "song_id": track.get("id", ""), "explicit": track.get("explicit", False), "publisher": "",
        "url": (track.get("external_urls") or {}).get("spotify", ""),
        "isrc": (track.get("external_ids") or {}).get("isrc"),
        "cover_url": images[0]["url"] if images else None, "copyright_text": None,
        "popularity": track.get("popularity"), "album_id": album.get("id"),
        "artist_id": (track.get("artists") or [{}])[0].get("id"), "album_type": album.get("album_type"),
    }


def write_missing_songs(missing: list[dict], path: Path, tried: Path) -> int:
    """Write the missing tracks as a .spotdl file, leaving out tracks spotdl already tried."""
    skip = set(tried.read_text().split()) if tried.exists() else set()
    songs = {}
    for track in missing:
        song = track.get("song")
        if song and song["url"] and song["url"] not in skip:
            songs.setdefault(song["url"], song)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(list(songs.values()), ensure_ascii=False))
    tmp.replace(path)
    return len(songs)


class RateLimited(RuntimeError):
    pass


def http_get(url: str, headers: dict = (), _retries: int = 3) -> dict:
    req = urllib.request.Request(url, headers=dict(headers))
    for attempt in range(_retries):
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                return json.loads(resp.read())
        except urllib.error.HTTPError as exc:
            if exc.code != 429:
                raise
            try:
                wait = int(exc.headers.get("Retry-After", 60))
            except ValueError:  # an HTTP date, not seconds
                wait = MAX_RETRY_WAIT + 1
            if wait > MAX_RETRY_WAIT or attempt == _retries - 1:
                raise RateLimited(f"Spotify rate limit: retry after {wait} s") from exc
            time.sleep(wait)


def http_post(url: str, body: dict | str, headers: dict = ()) -> dict:
    if isinstance(body, dict):
        data = urllib.parse.urlencode(body).encode()
        content_type = "application/x-www-form-urlencoded"
    else:
        data = body.encode() if isinstance(body, str) else body
        content_type = "application/json"
    req = urllib.request.Request(url, data=data, headers={"Content-Type": content_type,
                                                           **dict(headers)}, method="POST")
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


# --- Spotify -----------------------------------------------------------------

class SpotifyClient:
    def __init__(self, client_id: str, client_secret: str, refresh_token: str):
        self._client_id = client_id
        self._client_secret = client_secret
        self._refresh_token = refresh_token
        self._access_token: str = ""
        self._expires_at: float = 0

    def _basic_auth(self) -> str:
        creds = f"{self._client_id}:{self._client_secret}".encode()
        return "Basic " + base64.b64encode(creds).decode()

    def _ensure_token(self) -> None:
        if time.monotonic() < self._expires_at - 30:
            return
        resp = http_post(SPOTIFY_TOKEN_URL,
                         {"grant_type": "refresh_token", "refresh_token": self._refresh_token},
                         {"Authorization": self._basic_auth()})
        self._access_token = resp["access_token"]
        self._expires_at = time.monotonic() + resp["expires_in"]

    def _paginate(self, path: str, params: dict = ()) -> list[dict]:
        items: list[dict] = []
        url: str | None = f"{SPOTIFY_API}{path}"
        if params:
            url += "?" + urllib.parse.urlencode(params)
        while url:
            self._ensure_token()
            data = http_get(url, {"Authorization": f"Bearer {self._access_token}"})
            items.extend(data.get("items", []))
            url = data.get("next")
        return items

    def playlists(self) -> list[dict]:
        """Return all playlists the current user owns or follows."""
        return self._paginate("/me/playlists", {"limit": 50})

    def tracks(self, playlist_id: str) -> list[dict]:
        """Return track dicts with title, artists, album.

        Uses GET /playlists/{id} and paginates through the top-level 'items'
        object. The dedicated /tracks sub-endpoint is blocked in development mode.
        """
        result = []
        url: str | None = f"{SPOTIFY_API}/playlists/{playlist_id}?limit=100"
        first = True
        while url:
            self._ensure_token()
            data = http_get(url, {"Authorization": f"Bearer {self._access_token}"})
            page = (data.get("items") or data.get("tracks") or {}) if first else data
            first = False
            for item in page.get("items", []):
                track = item.get("item") or item.get("track")
                if not track or item.get("is_local") or track.get("is_local"):
                    continue
                if track.get("type") not in (None, "track"):
                    continue
                result.append({
                    "title": track.get("name", ""),
                    "artists": [a["name"] for a in track.get("artists", [])],
                    "album": track.get("album", {}).get("name", ""),
                    "song": spotdl_song(track),
                    "added_at": item.get("added_at") or "",
                })
            url = page.get("next")
        return result


def spotify_auth(client_id: str, client_secret: str) -> str:
    """Run the OAuth flow and return the refresh token."""
    state = secrets.token_hex(8)
    params = {
        "client_id": client_id,
        "response_type": "code",
        "redirect_uri": REDIRECT_URI,
        "scope": SCOPES,
        "state": state,
    }
    auth_url = SPOTIFY_AUTH_URL + "?" + urllib.parse.urlencode(params)
    print(f"\nOpen this URL in your browser:\n\n  {auth_url}\n")

    code_holder: list[str] = []
    done = threading.Event()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            parsed = urllib.parse.urlparse(self.path)
            q = urllib.parse.parse_qs(parsed.query)
            if q.get("state", [""])[0] != state:
                self.send_error(400, "State mismatch")
                return
            code_holder.append(q.get("code", [""])[0])
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"Authorization received. You can close this tab.")
            done.set()

    server = http.server.HTTPServer(("127.0.0.1", AUTH_PORT), Handler)

    def _serve():
        while not done.is_set():
            server.handle_request()

    t = threading.Thread(target=_serve, daemon=True)
    t.start()
    print(f"Waiting for authorization on port {AUTH_PORT}...")
    done.wait(timeout=120)
    server.server_close()

    if not code_holder:
        sys.exit("No authorization code received within 120 s.")

    basic = base64.b64encode(f"{client_id}:{client_secret}".encode()).decode()
    resp = http_post(SPOTIFY_TOKEN_URL,
                     {"grant_type": "authorization_code", "code": code_holder[0],
                      "redirect_uri": REDIRECT_URI},
                     {"Authorization": f"Basic {basic}"})
    return resp["refresh_token"]


class CachedSpotify:
    """Fetches a playlist's tracks only when its snapshot_id changes, and serves the
    last copy while Spotify rate limits. Each fill-loop round then costs one API call."""

    def __init__(self, client: SpotifyClient, path: Path):
        self._client = client
        self._path = path
        try:
            self._cache = json.loads(path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            self._cache = {"playlists": [], "tracks": {}}
        self._limited = False

    def _save(self) -> None:
        tmp = self._path.with_suffix(f".{os.getpid()}.tmp")
        tmp.write_text(json.dumps(self._cache))
        tmp.replace(self._path)

    def playlists(self) -> list[dict]:
        try:
            fresh = self._client.playlists()
        except RateLimited as err:
            if not self._cache["playlists"]:
                raise
            print(f"{err}; using cached playlists", file=sys.stderr)
            self._limited = True
            return self._cache["playlists"]
        self._cache = {**self._cache, "playlists": fresh}
        self._save()
        return fresh

    def tracks(self, playlist_id: str) -> list[dict]:
        snapshot = next((p.get("snapshot_id") for p in self._cache["playlists"]
                         if p.get("id") == playlist_id), None)
        entry = self._cache["tracks"].get(playlist_id)
        if entry and (self._limited or (snapshot and entry["snapshot"] == snapshot)):
            return entry["tracks"]
        try:
            fresh = self._client.tracks(playlist_id)
        except RateLimited:
            if not entry:
                raise
            self._limited = True
            return entry["tracks"]
        self._cache = {**self._cache, "tracks": {**self._cache["tracks"],
                                                 playlist_id: {"snapshot": snapshot,
                                                               "tracks": fresh}}}
        self._save()
        return fresh


# --- Navidrome ---------------------------------------------------------------

class Navidrome:
    def __init__(self, server: str, user: str, password: str):
        self.server = server.rstrip("/")
        self.user = user
        self.password = password

    def call(self, method: str, params: list[tuple] = ()) -> dict:
        salt = secrets.token_hex(8)
        token = hashlib.md5((self.password + salt).encode()).hexdigest()
        query = [("u", self.user), ("t", token), ("s", salt), ("v", "1.16.1"),
                 ("c", "spotify-sync"), ("f", "json"), *params]
        url = f"{self.server}/rest/{method}?" + urllib.parse.urlencode(query)
        with urllib.request.urlopen(url, timeout=30) as resp:
            body = json.load(resp)["subsonic-response"]
        if body["status"] != "ok":
            raise RuntimeError(f"{method}: {body.get('error', {}).get('message', 'failed')}")
        return body

    def find_track(self, track: dict) -> str | None:
        query = (f"{normalize(track['title'])} "
                 f"{normalize(track['artists'][0]) if track['artists'] else ''}").strip()
        found = self.call("search3", [("query", query), ("songCount", str(SEARCH_RESULTS)),
                                      ("artistCount", "0"), ("albumCount", "0")])
        songs = found.get("searchResult3", {}).get("song", [])
        for song in songs:
            if similar(track["title"], song.get("title", "")) < MATCH_THRESHOLD:
                continue
            nav_artist = song.get("artist", "")
            # Tags join artists with " / " (ID3v2.3), "feat.", "•", "&" or ","; keep the whole
            # string too, so a name like "Tyler, The Creator" still matches.
            nav_parts = [nav_artist, *(p.strip() for p in ARTIST_SEPARATORS.split(nav_artist))]
            if any(
                similar(a, part) >= MATCH_THRESHOLD
                for a in track["artists"]
                for part in nav_parts
            ):
                return song["id"]
        return None

    def save_playlist(self, name: str, song_ids: list[str]) -> None:
        # Subsonic createPlaylist sends song IDs as query parameters. URLs with 400+
        # songs exceed the ~8 KB nginx limit. Create with the first 200, then append
        # the rest via updatePlaylist.
        chunk_size = 200
        existing = self.call("getPlaylists").get("playlists", {}).get("playlist", [])
        playlist_id = next((p["id"] for p in existing if p["name"] == name), None)
        first = song_ids[:chunk_size]
        params = ([("playlistId", playlist_id)] if playlist_id else [("name", name)])
        resp = self.call("createPlaylist", params + [("songId", s) for s in first])
        if not playlist_id:
            playlist_id = (resp.get("playlist", {}).get("id")
                           or next((p["id"] for p in
                                    self.call("getPlaylists").get("playlists", {}).get("playlist", [])
                                    if p["name"] == name), None))
            if not playlist_id:
                raise RuntimeError(f"Could not create playlist {name!r}: no ID returned")
        for i in range(chunk_size, len(song_ids), chunk_size):
            chunk = song_ids[i:i + chunk_size]
            self.call("updatePlaylist",
                      [("playlistId", playlist_id)] + [("songIdToAdd", s) for s in chunk])


# --- Lidarr ------------------------------------------------------------------

class Lidarr:
    def __init__(self, api_key: str):
        self._key = api_key

    def _call(self, path: str, method: str = "GET", body: object = None) -> object:
        url = f"{LIDARR_URL}/{path}"
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data=data, method=method,
                                     headers={"Content-Type": "application/json",
                                              "X-Api-Key": self._key})
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                return json.loads(resp.read() or b"null")
        except urllib.error.HTTPError as err:
            raise RuntimeError(f"{method} {path}: {err.code} {err.read()[:200]}") from err
        except (urllib.error.URLError, TimeoutError) as err:
            raise RuntimeError(f"{method} {path}: {err}") from err

    def artist_ids_by_name(self) -> dict[str, int]:
        artists = self._call("artist") or []
        return {a["artistName"].lower(): a["id"] for a in artists}

    def quality_profile_id(self) -> int:
        profiles = self._call("qualityprofile") or []
        return profiles[0]["id"] if profiles else 1

    def metadata_profile_id(self) -> int:
        profiles = self._call("metadataprofile") or []
        return profiles[0]["id"] if profiles else 1

    def add_artist(self, name: str, quality_id: int, metadata_id: int) -> int | None:
        try:
            results = self._call(f"artist/lookup?term={urllib.parse.quote(name)}") or []
            if not results:
                return None
            artist = results[0]
            added = self._call("artist", "POST", {
                "foreignArtistId": artist["foreignArtistId"],
                "artistName": artist["artistName"],
                "qualityProfileId": quality_id,
                "metadataProfileId": metadata_id,
                "monitored": True,
                "monitorNewItems": "none",
                "rootFolderPath": ROOT_FOLDER,
                "addOptions": {"monitor": "future", "searchForMissingAlbums": False},
            })
            return added["id"]
        except RuntimeError as err:
            msg = str(err).lower()
            if "already exists" not in msg and "duplicate" not in msg and "already been added" not in msg:
                print(f"Warning: could not add {name!r} to Lidarr: {err}", file=sys.stderr)
            return None

    def refresh_artist(self, artist_id: int) -> None:
        """Trigger Lidarr to fetch the artist's discography in the background."""
        try:
            self._call("command", "POST", {"name": "RefreshArtist", "artistId": artist_id})
        except RuntimeError as err:
            print(f"Warning: artist refresh failed: {err}", file=sys.stderr)

    def want_album(self, artist_id: int, album_name: str) -> bool:
        """Mark the closest-matching album for an artist as monitored and trigger a search."""
        try:
            albums = self._call(f"album?artistId={artist_id}") or []
            if not albums:
                return False
            # Prefer an exact normalized title match over a stem-boosted one so that
            # "Fearless" does not arbitrarily win over "Fearless (Fan Edition)" by list order.
            def _rank(a: dict) -> tuple:
                t = a.get("title", "")
                return (normalize(album_name) == normalize(t), similar(album_name, t))
            best = max(albums, key=_rank)
            if similar(album_name, best.get("title", "")) < 0.7:
                return False
            if best.get("monitored"):
                return True
            body: dict = {
                "id": best["id"],
                "foreignAlbumId": best["foreignAlbumId"],
                "monitored": True,
            }
            if best.get("qualityProfileId") is not None:
                body["qualityProfileId"] = best["qualityProfileId"]
            self._call(f"album/{best['id']}", "PUT", body)
            self._call("command", "POST", {"name": "AlbumSearch", "albumIds": [best["id"]]})
            return True
        except RuntimeError as err:
            print(f"Warning: could not mark album {album_name!r} as wanted: {err}",
                  file=sys.stderr)
            return False


# --- sync logic --------------------------------------------------------------

def sync_playlist(playlist: dict, spotify: SpotifyClient | CachedSpotify,
                  navidrome: Navidrome, lidarr: Lidarr | None,
                  dry_run: bool, existing_artists: dict[str, int],
                  quality_id: int, metadata_id: int) -> dict:
    name = playlist.get("name", "Untitled")
    try:
        tracks = spotify.tracks(playlist["id"])
    except RateLimited as err:
        print(f"  Skipping {name!r}: {err}", file=sys.stderr)
        return {"name": name, "matched": 0, "total": 0, "missing": [], "skipped": True}
    except urllib.error.HTTPError as err:
        if err.code == 403:
            body = err.read().decode(errors="replace")[:200]
            print(f"  Skipping {name!r}: 403 {body}", file=sys.stderr)
            return {"name": name, "matched": 0, "total": 0, "missing": []}
        raise
    if not tracks:
        print(f"  {name!r}: no tracks returned from Spotify (empty or unavailable playlist)",
              file=sys.stderr)
        return {"name": name, "matched": 0, "total": 0, "missing": []}

    # Newest added first, like Spotify's "Date added" sort. The sort is stable, so cached tracks
    # without a date keep Spotify's custom order.
    tracks = sorted(tracks, key=lambda t: t.get("added_at", ""), reverse=True)
    song_ids = []
    missing: list[dict] = []
    for track in tracks:
        song_id = navidrome.find_track(track)
        if song_id:
            song_ids.append(song_id)
        else:
            missing.append(track)

    # Deduplicate while preserving order.
    song_ids = list(dict.fromkeys(song_ids))

    if not dry_run and song_ids:
        navidrome.save_playlist(name, song_ids)

    if lidarr and not dry_run:
        for track in missing:
            if not track["artists"]:
                continue
            artist_name = track["artists"][0]
            artist_key = artist_name.lower()
            lidarr_id = existing_artists.get(artist_key)
            if lidarr_id is None:
                new_id = lidarr.add_artist(artist_name, quality_id, metadata_id)
                # 0 marks a failed lookup, so the artist's other tracks do not wait 30 s again.
                existing_artists[artist_key] = new_id or 0
                if new_id:
                    lidarr.refresh_artist(new_id)
                    lidarr_id = new_id
            if lidarr_id and track["album"]:
                if not lidarr.want_album(lidarr_id, track["album"]):
                    print(f"  Lidarr: no album match for {track['album']!r} by {artist_name!r}",
                          file=sys.stderr)

    return {"name": name, "matched": len(song_ids), "total": len(tracks), "missing": missing}


# --- auth and main -----------------------------------------------------------

def cmd_auth() -> None:
    creds = load_env(SPOTIFY_CREDS)
    client_id = creds.get("SPOTIFY_CLIENT_ID") or os.environ.get("SPOTIFY_CLIENT_ID", "")
    client_secret = creds.get("SPOTIFY_CLIENT_SECRET") or os.environ.get("SPOTIFY_CLIENT_SECRET", "")
    if not client_id or not client_secret:
        sys.exit(f"Set SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET in {SPOTIFY_CREDS} first.")
    refresh_token = spotify_auth(client_id, client_secret)
    SPOTIFY_CREDS.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    save_env_key(SPOTIFY_CREDS, "SPOTIFY_REFRESH_TOKEN", refresh_token)
    print(f"Refresh token saved to {SPOTIFY_CREDS}.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--auth", action="store_true",
                        help="Run the one-time Spotify OAuth setup and exit.")
    parser.add_argument("--dry-run", action="store_true",
                        help="Match tracks but do not write playlists or add to Lidarr.")
    parser.add_argument("--no-lidarr", action="store_true",
                        help="Skip adding missing artists/albums to Lidarr.")
    parser.add_argument("--missing-out", type=Path,
                        help="Write missing tracks not yet tried by spotdl to this .spotdl file.")
    args = parser.parse_args()

    if args.auth:
        cmd_auth()
        return

    # Load Spotify credentials.
    spotify_env = load_env(SPOTIFY_CREDS)
    client_id = spotify_env.get("SPOTIFY_CLIENT_ID") or os.environ.get("SPOTIFY_CLIENT_ID", "")
    client_secret = (spotify_env.get("SPOTIFY_CLIENT_SECRET")
                     or os.environ.get("SPOTIFY_CLIENT_SECRET", ""))
    refresh_token = (spotify_env.get("SPOTIFY_REFRESH_TOKEN")
                     or os.environ.get("SPOTIFY_REFRESH_TOKEN", ""))
    if not all([client_id, client_secret, refresh_token]):
        sys.exit(f"Run --auth first, or set credentials in {SPOTIFY_CREDS}.")

    # Load Navidrome password.
    nd_env = load_env(NAVIDROME_CREDS)
    password = (nd_env.get("NAVIDROME_PASSWORD") or os.environ.get("NAVIDROME_PASSWORD", ""))
    if not password:
        sys.exit(f"Set NAVIDROME_PASSWORD in {NAVIDROME_CREDS} or in the environment.")

    spotify = CachedSpotify(SpotifyClient(client_id, client_secret, refresh_token),
                            SPOTIFY_CACHE)
    if not NAVIDROME_URL:
        sys.exit("Set NAVIDROME_URL in the environment (e.g. https://your-host/music).")
    navidrome = Navidrome(NAVIDROME_URL, NAVIDROME_USER, password)
    navidrome.call("ping")

    lidarr: Lidarr | None = None
    existing_artists: dict[str, int] = {}
    quality_id = metadata_id = 1
    if not args.no_lidarr:
        try:
            lidarr_env = load_env(LIDARR_CREDS)
            api_key = lidarr_env.get("LIDARR__AUTH__APIKEY", "")
            if api_key:
                lidarr = Lidarr(api_key)
                existing_artists = lidarr.artist_ids_by_name()
                quality_id = lidarr.quality_profile_id()
                metadata_id = lidarr.metadata_profile_id()
        except Exception as err:
            print(f"Lidarr not available, skipping: {err}", file=sys.stderr)

    playlists = spotify.playlists()
    total_missing: list[dict] = []
    skipped = False

    for pl in playlists:
        result = sync_playlist(pl, spotify, navidrome, lidarr, args.dry_run,
                               existing_artists, quality_id, metadata_id)
        prefix = "[dry-run] " if args.dry_run else ""
        print(f"{prefix}{result['name']}: {result['matched']}/{result['total']} in library, "
              f"{len(result['missing'])} missing")
        total_missing.extend(result["missing"])
        skipped = skipped or result.get("skipped", False)

    if total_missing:
        # Group by artist+album for a summary.
        by_album: dict[tuple, int] = defaultdict(int)
        for t in total_missing:
            key = (t["artists"][0] if t["artists"] else "?", t["album"])
            by_album[key] += 1
        print(f"\n{len(total_missing)} tracks not in library across "
              f"{len(playlists)} playlists. Top missing albums:")
        for (artist, album), count in sorted(by_album.items(), key=lambda x: -x[1])[:10]:
            print(f"  {count:3}  {artist} - {album}")
        if lidarr and not args.dry_run:
            print("Missing artists and albums added to Lidarr for download.")
    # A skipped playlist's tracks are unknown; no file keeps the fill loop from ending early.
    if args.missing_out and not skipped:
        count = write_missing_songs(total_missing, args.missing_out, SPOTDL_TRIED)
        print(f"{count} missing tracks written to {args.missing_out}")


if __name__ == "__main__":
    main()

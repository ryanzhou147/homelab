#!/usr/bin/env python3
"""Download a full album from Spotify/YouTube Music via spotdl.
Usage: spotdl-album.py "Artist" "Album Title"
Reads credentials from $DATA/secrets/spotify.env.
"""
import json
import os
import subprocess
import sys
import urllib.parse
import urllib.request
from pathlib import Path

DATA = Path(os.environ.get("DATA", Path.home() / "homelab-data"))
MUSIC = Path.home() / "homelab-media" / "music"


def load_env(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    if not path.exists():
        return result
    for line in path.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, _, v = line.partition("=")
            result[k.strip()] = v.strip().strip("\"'")
    return result


def refresh_token(client_id: str, client_secret: str, refresh: str) -> str:
    body = urllib.parse.urlencode({
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "client_id": client_id,
        "client_secret": client_secret,
    }).encode()
    req = urllib.request.Request(
        "https://accounts.spotify.com/api/token",
        data=body,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.loads(resp.read())["access_token"]


def search_album(token: str, artist: str, album: str) -> str | None:
    query = urllib.parse.quote(f"artist:{artist} album:{album}")
    url = f"https://api.spotify.com/v1/search?q={query}&type=album&limit=5"
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req, timeout=20) as resp:
        data = json.loads(resp.read())
    items = data.get("albums", {}).get("items", [])
    if not items:
        # Fallback: looser search
        query2 = urllib.parse.quote(f"{artist} {album}")
        url2 = f"https://api.spotify.com/v1/search?q={query2}&type=album&limit=5"
        req2 = urllib.request.Request(url2, headers={"Authorization": f"Bearer {token}"})
        with urllib.request.urlopen(req2, timeout=20) as resp2:
            data = json.loads(resp2.read())
        items = data.get("albums", {}).get("items", [])
    if not items:
        return None
    best = items[0]
    return best["external_urls"]["spotify"]


def main() -> int:
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} 'Artist' 'Album'", file=sys.stderr)
        return 1

    artist = sys.argv[1]
    album = sys.argv[2]

    env = load_env(DATA / "secrets" / "spotify.env")
    client_id = env.get("SPOTIFY_CLIENT_ID", "")
    client_secret = env.get("SPOTIFY_CLIENT_SECRET", "")
    refresh = env.get("SPOTIFY_REFRESH_TOKEN", "")

    if not all([client_id, client_secret, refresh]):
        print("Missing Spotify credentials in spotify.env", file=sys.stderr)
        return 1

    print(f"Searching Spotify for: {artist} - {album}")
    token = refresh_token(client_id, client_secret, refresh)
    url = search_album(token, artist, album)

    if not url:
        print(f"Not found on Spotify: {artist} - {album}", file=sys.stderr)
        return 1

    print(f"Found: {url}")

    # Download to artist directory with spotdl.
    artist_dir = MUSIC / artist
    artist_dir.mkdir(parents=True, exist_ok=True)
    cmd = [
        "spotdl", "download", url,
        "--client-id", client_id,
        "--client-secret", client_secret,
        "--output", "{album} ({year})/{track-number} - {title}.{output-ext}",
        "--format", "mp3",
        "--bitrate", "320k",
        "--threads", "4",
    ]
    result = subprocess.run(cmd, cwd=str(artist_dir))
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())

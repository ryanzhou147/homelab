#!/usr/bin/env python3
"""Rebuilds Spotify playlists (Exportify CSV exports) as Navidrome playlists.

Each Spotify track is matched to a song in your Navidrome library by title and artist. Tracks not
in the library yet go to missing-albums.csv, a shopping list grouped by album with a Bandcamp
search link. Running it again after adding music updates the same playlists, so they fill in
over time.

    ./scripts/import-spotify-playlists.py ~/Downloads/spotify-export

Asks for your password; nothing is stored. NAVIDROME_PASSWORD in the environment skips the prompt.
"""

import csv
import getpass
import hashlib
import json
import os
import re
import secrets
import sys
import urllib.parse
import urllib.request
from collections import Counter
from difflib import SequenceMatcher
from pathlib import Path

SERVER = os.environ.get("NAVIDROME_URL", "https://your-pc.your-tailnet.ts.net/music")
USER = os.environ.get("NAVIDROME_USER", "admin")
MATCH_THRESHOLD = 0.85   # title and artist similarity needed to count as the same song
SEARCH_RESULTS = 20
SHOPPING_LIST = "missing-albums.csv"

# Version and feature tags Spotify adds to titles that library files usually don't have.
NOISE = re.compile(
    r"\s*[\(\[-][^)\]]*\b(remaster(ed)?|feat\.?|ft\.?|with|version|edit|mono|stereo|live|"
    r"deluxe|bonus|explicit)\b[^)\]]*[\)\]]?",
    re.IGNORECASE,
)


def normalize(text: str) -> str:
    """Lowercase, without version tags and punctuation, so the same song compares equal."""
    text = NOISE.sub("", text or "").lower()
    text = re.sub(r"[^\w\s]", " ", text)
    return " ".join(text.split())


def artists_of(field: str) -> list[str]:
    """Exportify separates artists with ';' (older exports used ', ')."""
    separator = ";" if ";" in field else ", "
    return [a.strip() for a in field.split(separator) if a.strip()]


def similar(a: str, b: str) -> float:
    return SequenceMatcher(None, normalize(a), normalize(b)).ratio()


def is_match(track: dict, song: dict) -> bool:
    if similar(track["title"], song.get("title", "")) < MATCH_THRESHOLD:
        return False
    song_artist = song.get("artist", "")
    return any(similar(a, song_artist) >= MATCH_THRESHOLD or normalize(a) in normalize(song_artist)
               for a in track["artists"])


def read_playlist(path: Path) -> list[dict]:
    with path.open(newline="", encoding="utf-8-sig") as f:
        rows = list(csv.DictReader(f))
    return [{"title": r.get("Track Name", ""), "artists": artists_of(r.get("Artist Name(s)", "")),
             "album": r.get("Album Name", "")} for r in rows if r.get("Track Name")]


class Navidrome:
    """The Subsonic API, signed with a salted token so the password never travels."""

    def __init__(self, server: str, user: str, password: str):
        self.server, self.user, self.password = server.rstrip("/"), user, password

    def call(self, method: str, params: list[tuple[str, str]] = ()) -> dict:
        salt = secrets.token_hex(8)
        token = hashlib.md5((self.password + salt).encode()).hexdigest()
        query = [("u", self.user), ("t", token), ("s", salt), ("v", "1.16.1"),
                 ("c", "spotify-import"), ("f", "json"), *params]
        url = f"{self.server}/rest/{method}?{urllib.parse.urlencode(query)}"
        with urllib.request.urlopen(url, timeout=30) as response:
            body = json.load(response)["subsonic-response"]
        if body["status"] != "ok":
            raise RuntimeError(f"{method}: {body.get('error', {}).get('message', 'failed')}")
        return body

    def find(self, track: dict) -> str | None:
        query = f"{normalize(track['title'])} {normalize(track['artists'][0]) if track['artists'] else ''}"
        found = self.call("search3", [("query", query.strip()), ("songCount", str(SEARCH_RESULTS)),
                                      ("artistCount", "0"), ("albumCount", "0")])
        songs = found.get("searchResult3", {}).get("song", [])
        return next((s["id"] for s in songs if is_match(track, s)), None)

    def save_playlist(self, name: str, song_ids: list[str]) -> None:
        """Creates the playlist, or replaces the songs of the one with the same name."""
        existing = self.call("getPlaylists").get("playlists", {}).get("playlist", [])
        playlist_id = next((p["id"] for p in existing if p["name"] == name), None)
        params = [("playlistId", playlist_id)] if playlist_id else [("name", name)]
        self.call("createPlaylist", params + [("songId", s) for s in song_ids])


def write_shopping_list(missing: Counter, folder: Path) -> Path:
    path = folder / SHOPPING_LIST
    with path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(["songs you need", "artist", "album", "bandcamp search"])
        for (artist, album), count in missing.most_common():
            query = urllib.parse.quote_plus(f"{artist} {album}")
            writer.writerow([count, artist, album, f"https://bandcamp.com/search?q={query}&item_type=a"])
    return path


def main() -> None:
    if len(sys.argv) != 2 or not Path(sys.argv[1]).is_dir():
        sys.exit("Usage: import-spotify-playlists.py <folder with Exportify CSV files>")
    folder = Path(sys.argv[1]).expanduser()
    files = sorted(folder.glob("*.csv"))
    files = [f for f in files if f.name != SHOPPING_LIST]
    if not files:
        sys.exit(f"No Exportify CSV files in {folder}.")

    password = os.environ.get("NAVIDROME_PASSWORD") or getpass.getpass(f"Password for {USER}: ")
    navidrome = Navidrome(SERVER, USER, password)
    navidrome.call("ping")

    missing: Counter = Counter()
    for path in files:
        tracks = read_playlist(path)
        found = [(t, navidrome.find(t)) for t in tracks]
        song_ids = list(dict.fromkeys(i for _, i in found if i))
        for track, song_id in found:
            if not song_id:
                missing[(track["artists"][0] if track["artists"] else "", track["album"])] += 1
        name = path.stem.replace("_", " ")
        navidrome.save_playlist(name, song_ids)
        print(f"{name}: {len(song_ids)} of {len(tracks)} songs in your library")

    if missing:
        shopping = write_shopping_list(missing, folder)
        print(f"\n{sum(missing.values())} songs are not in your library yet. Albums that fill the most:")
        for (artist, album), count in missing.most_common(10):
            print(f"  {count:3}  {artist} - {album}")
        print(f"Full list with Bandcamp links: {shopping}")


if __name__ == "__main__":
    main()

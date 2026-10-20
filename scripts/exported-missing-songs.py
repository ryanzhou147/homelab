#!/usr/bin/env python3
"""Writes a spotdl list of the songs in Exportify CSV playlists that are not in Navidrome.

The automatic sync cannot read Liked Songs or Saved Albums, so their missing songs never reach
the fill loop. The song details come from the CSV, because Spotify blocks track lookups for this
app. Songs in spotdl-tried.txt are left out; the caller adds the list's URLs there after spotdl.

    ./scripts/exported-missing-songs.py ~/Downloads/spotify-export ~/homelab-data/exported.spotdl
"""

import csv
import importlib.util
import os
import sys
from pathlib import Path

SCRIPTS = Path(__file__).parent
DATA = Path(os.environ.get("DATA", Path.home() / "homelab-data"))


def _load(name: str):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)  # type: ignore[union-attr]
    return module


def song_from_row(row: dict) -> dict:
    """spotdl's Song fields from one Exportify row. The CSV has no track number or cover."""
    artists = [a.strip() for a in row.get("Artist Name(s)", "").split(";") if a.strip()]
    track_id = row.get("Track URI", "").rsplit(":", 1)[-1]
    date = row.get("Release Date") or "0"
    return {
        "name": row.get("Track Name", ""), "artists": artists, "artist": artists[0] if artists else "",
        "genres": [], "disc_number": 1, "disc_count": 1, "album_name": row.get("Album Name", ""),
        "album_artist": artists[0] if artists else "",
        "duration": round(int(row.get("Duration (ms)") or 0) / 1000), "year": int(date[:4]),
        "date": date, "track_number": 1, "tracks_count": 1, "song_id": track_id,
        "explicit": row.get("Explicit") == "true",
        "publisher": row.get("Record Label", ""),
        "url": f"https://open.spotify.com/track/{track_id}" if track_id else "",
        "isrc": row.get("ISRC") or None, "cover_url": None, "copyright_text": None,
        "popularity": None, "album_id": None, "artist_id": None, "album_type": None,
    }


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit("Usage: exported-missing-songs.py <folder with Exportify CSV files> <out.spotdl>")
    sync, imp = _load("sync-spotify"), _load("import-spotify-playlists")
    password = sync.load_env(DATA / "secrets" / "navidrome.env").get("NAVIDROME_PASSWORD", "")
    navidrome = imp.Navidrome(imp.SERVER, imp.USER, password)
    navidrome.call("ping")
    missing, seen = [], set()
    for path in sorted(Path(sys.argv[1]).expanduser().glob("*.csv")):
        with path.open(newline="", encoding="utf-8-sig") as f:
            # Episode rows stay: in these exports they are unreleased songs uploaded as podcasts.
            rows = [r for r in csv.DictReader(f) if r.get("Track Name") and r.get("Track URI")]
        for row in rows:
            if row["Track URI"] in seen:
                continue
            seen.add(row["Track URI"])
            track = {"title": row["Track Name"], "artists": imp.artists_of(row.get("Artist Name(s)", "")),
                     "album": row.get("Album Name", "")}
            if not navidrome.find(track):
                missing.append({"song": song_from_row(row)})
    count = sync.write_missing_songs(missing, Path(sys.argv[2]), DATA / "spotdl-tried.txt")
    print(f"{len(missing)} songs not in the library; {count} not tried before, written to {sys.argv[2]}")


if __name__ == "__main__":
    main()

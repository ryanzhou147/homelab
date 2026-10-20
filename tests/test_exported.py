"""Tests for exported-missing-songs.py."""
import importlib.util
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "exported_missing_songs",
    Path(__file__).parent.parent / "scripts" / "exported-missing-songs.py",
)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)  # type: ignore[union-attr]


def test_song_from_exportify_row_has_what_spotdl_needs():
    song = _mod.song_from_row({
        "Track URI": "spotify:track:abc123", "Track Name": "We Found Love",
        "Artist Name(s)": "Rihanna;Calvin Harris", "Album Name": "Talk That Talk",
        "Release Date": "2011-11-18", "Duration (ms)": "215226", "ISRC": "USUM71111621",
    })
    assert song["url"] == "https://open.spotify.com/track/abc123"
    assert song["artists"] == ["Rihanna", "Calvin Harris"] and song["album_artist"] == "Rihanna"
    assert (song["duration"], song["year"], song["isrc"]) == (215, 2011, "USUM71111621")


def test_year_only_release_date():
    assert _mod.song_from_row({"Track URI": "spotify:track:x", "Release Date": "1994"})["year"] == 1994

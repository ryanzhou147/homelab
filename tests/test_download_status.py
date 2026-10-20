"""Tests for download-status.py."""
import importlib.util
import json
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "download_status", Path(__file__).parent.parent / "scripts" / "download-status.py")
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)  # type: ignore[union-attr]


def test_counts_only_the_last_run(tmp_path):
    songs = tmp_path / "list.spotdl"
    songs.write_text(json.dumps([{}] * 5))
    log = (f"Processing query: /old.spotdl\nDownloaded \"Old - Song\": url\n"
           f"Processing query: {songs}   \nDownloaded \"A - One\": url\n"
           "YouTube Music returned no usable results for b - two\nafter 3 attempts\n"
           "LookupError: No results found for song: b - two\n"
           "YouTube Music returned no usable results for c - three\n"
           "Downloaded \"C - Three\": url\n"
           "Skipping D - Four (file already exists) (duplicate)\n")
    assert _mod.status(log) == {"done": 2, "failed": 1, "total": 5, "left": 1, "percent": 80,
                                "last": "C - Three"}


def test_log_without_a_run():
    assert _mod.status("nothing yet")["total"] == 0

"""Tests for merge-split-albums.py."""
import importlib.util
import json
import sqlite3
import sys
from pathlib import Path

from mutagen.id3 import ID3, TALB, TXXX

_spec = importlib.util.spec_from_file_location(
    "merge_split_albums",
    Path(__file__).parent.parent / "scripts" / "merge-split-albums.py",
)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)  # type: ignore[union-attr]

split_albums = _mod.split_albums


def _db(albums, files, path=":memory:"):
    """albums: (id, artist, name, year, mbid, songs); files: (path, title, album, track, seconds)."""
    db = sqlite3.connect(path)
    db.execute("create table album (id, album_artist, name, max_year, mbz_album_id, song_count, missing)")
    db.execute("create table media_file (path, title, album_id, disc_number, track_number, duration, missing)")
    db.executemany("insert into album values (?, ?, ?, ?, ?, ?, 0)", albums)
    db.executemany("insert into media_file values (?, ?, ?, 0, ?, ?, 0)", files)
    db.commit()
    return db


def test_tagged_copy_is_kept_and_its_songs_are_duplicates():
    """The Lidarr copy stays even when smaller; spotdl songs it has are duplicates."""
    db = _db(
        [("a", "Nas", "Hip Hop Is Dead", 2006, "", 3), ("b", "nas", "hip hop is dead", 2006, "mb1", 2)],
        [("s/1.mp3", "Intro", "a", 1, 100), ("s/2.mp3", "Carry On", "a", 3, 200), ("s/3.mp3", "Hope", "a", 4, 150),
         ("l/1.flac", "intro", "b", 1, 98), ("l/2.flac", "Can't Forget", "b", 2, 210)],
    )
    assert split_albums(db) == [("l/1.flac", ["s/2.mp3", "s/3.mp3"], ["s/1.mp3"])]


def test_same_track_and_length_is_a_duplicate_even_with_another_title():
    """'Payday' and 'Payday (feat. Young Thug)' at track 3 with 2 s between them are one song."""
    db = _db(
        [("a", "Doja Cat", "Planet Her", 2021, "mb1", 1), ("b", "Doja Cat", "Planet Her", 2021, "", 2)],
        [("l/3.mp3", "Payday (feat. Young Thug)", "a", 3, 211),
         ("s/3.mp3", "Payday", "b", 3, 213), ("s/4.mp3", "Get Into It (Yuh)", "b", 4, 138)],
    )
    assert split_albums(db) == [("l/3.mp3", ["s/4.mp3"], ["s/3.mp3"])]


def test_song_in_two_extra_copies_is_retagged_once():
    """With 3 copies, a song that only the 2 smaller copies have joins the album once."""
    db = _db(
        [("a", "Metro Boomin", "Heroes", 2022, "mb1", 1), ("b", "Metro Boomin", "Heroes", 2022, "", 1),
         ("c", "Metro Boomin", "Heroes", 2022, "", 1)],
        [("l/1.mp3", "Intro", "a", 1, 60),
         ("s/2.mp3", "On Time", "b", 2, 169), ("t/2.mp3", "On Time (with John Legend)", "c", 2, 170)],
    )
    assert split_albums(db) == [("l/1.mp3", ["s/2.mp3"], ["t/2.mp3"])]


def test_two_interludes_are_two_songs():
    db = _db(
        [("a", "Nas", "Album", 2006, "mb1", 1), ("b", "Nas", "Album", 2006, "", 1)],
        [("l/4.mp3", "Interlude", "a", 4, 60), ("s/9.mp3", "Interlude", "b", 9, 95)],
    )
    assert split_albums(db) == [("l/4.mp3", ["s/9.mp3"], [])]


def test_remix_is_not_a_duplicate_of_the_original():
    db = _db(
        [("a", "2Pac", "Loyal", 2004, "mb1", 1), ("b", "2Pac", "Loyal", 2004, "", 1)],
        [("l/15.mp3", "Hennessey", "a", 15, 250), ("s/15.mp3", "Hennessey - Red Spyda Remix", "b", 15, 252)],
    )
    assert split_albums(db) == [("l/15.mp3", ["s/15.mp3"], [])]


def test_reissue_from_another_year_joins_when_it_shares_a_song():
    db = _db(
        [("a", "Lady Gaga", "The Fame", 2008, "mb1", 2), ("b", "Lady Gaga", "The Fame", 2019, "", 2)],
        [("l/1.mp3", "Just Dance", "a", 1, 242), ("l/2.mp3", "LoveGame", "a", 2, 216),
         ("s/1.mp3", "Just Dance (feat. Colby O'Donis)", "b", 1, 244), ("s/3.mp3", "Paparazzi", "b", 3, 208)],
    )
    assert split_albums(db) == [("l/1.mp3", ["s/3.mp3"], ["s/1.mp3"])]


def test_two_skits_of_similar_length_are_two_songs():
    """A repeated title must match on track number; length alone is not enough."""
    db = _db(
        [("a", "X", "Tape", 2020, "mb1", 2), ("b", "X", "Tape", 2020, "", 2)],
        [("l/5.mp3", "Skit", "a", 5, 45), ("l/6.mp3", "Song", "a", 6, 200),
         ("s/5.mp3", "Skit", "b", 5, 45), ("s/11.mp3", "Skit", "b", 11, 49)],
    )
    assert split_albums(db) == [("l/5.mp3", ["s/11.mp3"], ["s/5.mp3"])]


def test_same_name_in_other_year_is_a_different_album():
    db = _db(
        [("a", "Weezer", "Weezer", 1994, "", 10), ("b", "Weezer", "Weezer", 2001, "", 10)],
        [("1/x.mp3", "Buddy Holly", "a", 1, 160), ("2/x.mp3", "Hash Pipe", "b", 1, 190)],
    )
    assert split_albums(db) == []


def test_two_musicbrainz_releases_are_left_alone():
    """A single and its album, both from Lidarr, keep their own files."""
    db = _db(
        [("a", "Joey", "2000", 2022, "mb-album", 14), ("b", "Joey", "2000", 2022, "mb-single", 1)],
        [("a/1.flac", "Where I Belong", "a", 1, 200), ("b/1.flac", "Where I Belong", "b", 1, 200)],
    )
    assert split_albums(db) == []


def _mp3(path, frames):
    """A tiny valid MP3: ID3 frames, then silent 128 kbps MPEG frames."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes((b"\xff\xfb\x90\x64" + bytes(413)) * 10)
    tags = ID3()
    for frame in frames:
        tags.add(frame)
    tags.save(path)
    return path


def test_retag_copies_upper_case_musicbrainz_id(tmp_path):
    """A keeper with TXXX:MUSICBRAINZ_ALBUMID passes the ID on, and the old spelling goes away."""
    keep = _mp3(tmp_path / "keep.mp3", [TALB(text="Disorder"), TXXX(desc="MUSICBRAINZ_ALBUMID", text="mb1")])
    other = _mp3(tmp_path / "other.mp3", [TALB(text="DISORDER"), TXXX(desc="MUSICBRAINZ_ALBUMID", text="mb2")])
    new = _mod.album_tags(keep)
    assert new == {"album": ["Disorder"], "musicbrainz_albumid": ["mb1"]}
    _mod.retag(other, new)
    ids = {k: str(v) for k, v in ID3(other).items() if k.startswith("TXXX")}
    assert ids == {"TXXX:MusicBrainz Album Id": "mb1"}
    assert str(ID3(other)["TALB"]) == "Disorder"


def test_main_moves_duplicates_retags_the_rest_and_logs_both(tmp_path, monkeypatch):
    music, dups = tmp_path / "music", tmp_path / "dups"
    _mp3(music / "l/1.mp3", [TALB(text="Heroes"), TXXX(desc="MusicBrainz Album Id", text="mb1")])
    _mp3(music / "s/1.mp3", [TALB(text="HEROES")])
    _mp3(music / "s/2.mp3", [TALB(text="HEROES")])
    _db(
        [("a", "Metro", "Heroes", 2022, "mb1", 1), ("b", "Metro", "Heroes", 2022, "", 2)],
        [("l/1.mp3", "Intro", "a", 1, 60), ("s/1.mp3", "Intro", "b", 1, 61), ("s/2.mp3", "Outro", "b", 2, 90)],
        tmp_path / "navidrome.db",
    ).close()
    for name, value in [("MUSIC", music), ("DUPLICATES", dups), ("NAVIDROME_DB", tmp_path / "navidrome.db"),
                        ("UNDO_LOG", tmp_path / "undo.jsonl")]:
        monkeypatch.setattr(_mod, name, value)
    monkeypatch.setattr(sys, "argv", ["merge-split-albums.py", "--apply"])

    _mod.main()

    assert not (music / "s/1.mp3").exists() and (dups / "s/1.mp3").exists()
    assert _mod.album_tags(music / "s/2.mp3") == {"album": ["Heroes"], "musicbrainz_albumid": ["mb1"]}
    log = [json.loads(line) for line in (tmp_path / "undo.jsonl").read_text().splitlines()]
    assert log == [{"path": "s/1.mp3", "moved_to": str(dups / "s/1.mp3")},
                   {"path": "s/2.mp3", "old": {"album": ["HEROES"]}}]


def test_untagged_copy_merges_while_a_single_with_its_own_id_stays():
    db = _db(
        [("a", "Joey", "2000", 2022, "mb-album", 2), ("b", "Joey", "2000", 2022, "mb-single", 1),
         ("c", "Joey", "2000", 2022, "", 1)],
        [("a/1.flac", "Make Me Feel", "a", 2, 200), ("b/1.flac", "Make Me Feel", "b", 1, 200),
         ("c/9.mp3", "Survival Tactics", "c", 9, 220)],
    )
    assert split_albums(db) == [("a/1.flac", ["c/9.mp3"], [])]

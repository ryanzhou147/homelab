#!/usr/bin/env python3
"""Merge albums that Navidrome shows twice because their files have different album tags.

Lidarr files have a MusicBrainz album ID and spotdl files do not, so one album folder can show as
two albums. For each album that Navidrome lists more than once with the same album artist, name
and year, this keeps the copy with a MusicBrainz ID (else the one with the most songs). Songs that
copy already has move to $MEDIA/music-duplicates; the other files get its album tags, and
Navidrome merges them at its next scan. Old tags and moves go to $DATA/merge-split-albums.jsonl.
Usage: merge-split-albums.py [--apply]   (without --apply it only prints the plan)
"""
import json
import os
import re
import sqlite3
import sys
from collections import Counter
from pathlib import Path

import mutagen
from mutagen.easyid3 import EasyID3

DATA = Path(os.environ.get("DATA", Path.home() / "homelab-data"))
MEDIA = Path(os.environ.get("MEDIA", Path.home() / "homelab-media"))
MUSIC = MEDIA / "music"
DUPLICATES = MEDIA / "music-duplicates"
NAVIDROME_DB = DATA / "navidrome/navidrome.db"
UNDO_LOG = DATA / "merge-split-albums.jsonl"
# MP3 keeps the release date in TDRL, which mutagen's easy names do not cover.
EasyID3.RegisterTextKey("releasedate", "TDRL")
# Some taggers write TXXX:MUSICBRAINZ_ALBUMID where mutagen expects TXXX:MusicBrainz Album Id.
# Navidrome reads both, so read both and write only mutagen's.
UPPER_TXXX = ("musicbrainz_albumid", "musicbrainz_albumartistid", "musicbrainz_releasegroupid",
              "originaldate", "releasedate")
for _key in UPPER_TXXX:
    EasyID3.RegisterTXXXKey(f"{_key}_upper", _key.upper())
# The tags Navidrome reads to tell albums apart (its PIDAlbum setting), in mutagen's easy names.
ALBUM_TAGS = (
    "album", "albumartist", "albumartistsort", "date", "originaldate", "releasedate",
    "musicbrainz_albumid", "musicbrainz_albumartistid", "musicbrainz_releasegroupid",
)


# Two files hold the same song when their base titles match and so do the disc and track numbers.
# When the title occurs once on each side, lengths that differ by less than this also count:
# spotdl's YouTube copies run a few seconds longer and can have other track numbers.
SAME_SONG_SECONDS = 8

VERSION_WORDS = ("remix", "live", "acoustic", "instrumental", "demo")
Song = tuple[str, str, int, int, float]  # path, title, disc, track, seconds


def base_title(title: str) -> str:
    """'Payday (feat. Young Thug)' -> 'payday': the title before any (, [ or ' - ' tag."""
    lower = title.lower()
    head = re.sub(r"[^0-9a-z]", "", re.split(r"[(\[]| - ", lower)[0]) or lower
    # A remix or live take of a song is another song.
    return head + "".join(w for w in VERSION_WORDS if re.search(rf"\b{w}\b", lower))


def sort_copies(kept: list[Song], others: list[Song]) -> tuple[list[str], list[str]]:
    """Split the other copies' files into (files to retag, songs the album already has)."""
    kept_count = Counter(base_title(s[1]) for s in kept)
    other_count = Counter(base_title(s[1]) for s in others)
    seen: dict[str, list[tuple[int, int, float]]] = {}
    for _, title, disc, track, seconds in kept:
        seen.setdefault(base_title(title), []).append((disc, track, seconds))
    to_retag, duplicates = [], []
    for path, title, disc, track, seconds in others:
        base = base_title(title)
        by_length = kept_count[base] <= 1 and other_count[base] == 1
        matches = seen.setdefault(base, [])
        if any((d, t) == (disc, track) or (by_length and abs(s - seconds) < SAME_SONG_SECONDS)
               for d, t, s in matches):
            duplicates.append(path)
        else:
            to_retag.append(path)
            matches.append((disc, track, seconds))
    return to_retag, duplicates


def _songs(db: sqlite3.Connection, album_id: str) -> list[Song]:
    return db.execute("""select path, title, max(disc_number, 1), track_number, duration
        from media_file where album_id = ? and missing = 0 order by path""", (album_id,)).fetchall()


def split_albums(db: sqlite3.Connection) -> list[tuple[str, list[str], list[str]]]:
    """Return (a file of the kept copy, files to retag, duplicate files) for each split album.

    The other copies of an album name join the kept copy when they have the same year, or share a
    song with it (a reissue). A copy with another MusicBrainz release ID, such as a single, stays.
    """
    groups = db.execute("""select lower(album_artist), lower(name) from album where missing = 0
        group by 1, 2 having count(*) > 1""").fetchall()
    plan = []
    for key in groups:
        albums = db.execute("""
            select id, coalesce(mbz_album_id, ''), max_year from album
            where lower(album_artist) = ? and lower(name) = ? and missing = 0
            order by coalesce(mbz_album_id, '') = '', song_count desc, id""", key).fetchall()
        (kept_id, kept_mbid, kept_year), rest = albums[0], albums[1:]
        kept = _songs(db, kept_id)
        others = []
        for album_id, mbid, year in rest:
            songs = _songs(db, album_id)
            if mbid and mbid != kept_mbid:
                continue
            if year == kept_year or sort_copies(kept, songs)[1]:
                others += songs
        to_retag, duplicates = sort_copies(kept, others)
        if kept and others:
            plan.append((kept[0][0], to_retag, duplicates))
    return plan


def _names(key: str) -> tuple[str, ...]:
    return (key, f"{key}_upper") if key in UPPER_TXXX else (key,)


def album_tags(path: Path) -> dict[str, list[str]]:
    tags = mutagen.File(path, easy=True)
    found = {}
    for k in ALBUM_TAGS:
        values = next((list(tags[n]) for n in _names(k) if tags.tags is not None and n in tags), None)
        if values:
            found[k] = values
    return found


def retag(path: Path, new: dict[str, list[str]]) -> dict[str, list[str]]:
    """Give the file exactly the album tags in new, and return its old album tags."""
    tags = mutagen.File(path, easy=True)
    if tags.tags is None:
        tags.add_tags()
    old = {n: list(tags[n]) for k in ALBUM_TAGS for n in _names(k) if n in tags}
    for k in ALBUM_TAGS:
        for n in _names(k):
            if n in tags:
                del tags[n]
        if k in new:
            tags[k] = new[k]
    tags.save()
    return old


def main() -> None:
    apply = "--apply" in sys.argv[1:]
    db = sqlite3.connect(f"file:{NAVIDROME_DB}?mode=ro", uri=True)
    plan = split_albums(db)
    retagged = moved = 0
    with UNDO_LOG.open("a", buffering=1) if apply else open(os.devnull, "w") as undo:
        for keep, to_retag, duplicates in plan:
            try:
                new = album_tags(MUSIC / keep)
            except (OSError, mutagen.MutagenError, AttributeError) as e:
                print(f"  skipped {Path(keep).parent}: {e}", file=sys.stderr)
                continue
            if "album" not in new:
                print(f"  skipped {Path(keep).parent}: {keep} has no album tag", file=sys.stderr)
                continue
            print(f"{Path(keep).parent}: retag {len(to_retag)}, move {len(duplicates)}")
            if not apply:
                continue
            for rel in duplicates:
                target = DUPLICATES / rel
                if target.exists():
                    print(f"  skipped {rel}: {target} exists", file=sys.stderr)
                    continue
                try:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    (MUSIC / rel).rename(target)
                except OSError as e:
                    print(f"  skipped {rel}: {e}", file=sys.stderr)
                    continue
                undo.write(json.dumps({"path": rel, "moved_to": str(target)}) + "\n")
                moved += 1
            for rel in to_retag:
                try:
                    old = retag(MUSIC / rel, new)
                except (OSError, mutagen.MutagenError, AttributeError) as e:
                    print(f"  skipped {rel}: {e}", file=sys.stderr)
                    continue
                undo.write(json.dumps({"path": rel, "old": old}) + "\n")
                retagged += 1
    print(f"{len(plan)} split albums: {retagged} files retagged, {moved} duplicates moved"
          + ("" if apply else " (dry run)"))


if __name__ == "__main__":
    main()

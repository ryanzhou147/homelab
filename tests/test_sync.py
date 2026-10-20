"""Tests for sync-spotify.py helpers."""
import importlib.util
import json
from pathlib import Path

# Load the script as a module without executing the if __name__ == "__main__" block.
_spec = importlib.util.spec_from_file_location(
    "sync_spotify",
    Path(__file__).parent.parent / "scripts" / "sync-spotify.py",
)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)  # type: ignore[union-attr]

similar = _mod.similar
MATCH_THRESHOLD = _mod.MATCH_THRESHOLD


def test_pmedia_suffix_matches():
    """A (PMEDIA) tag must not drop the score below the match threshold."""
    assert similar("United in Grief (PMEDIA)", "United in Grief") >= MATCH_THRESHOLD


def test_short_prefix_does_not_match_unrelated_title():
    """'Red' must not match 'Red Dead Redemption' just because it is a prefix."""
    assert similar("Red", "Red Dead Redemption") < MATCH_THRESHOLD


def test_feat_on_one_side_pmedia_on_other():
    """Same song tagged '(feat. Drake)' on one side and '(PMEDIA)' on the other must match."""
    assert similar("One More Night (feat. Drake)", "One More Night (PMEDIA)") >= MATCH_THRESHOLD


class _FakeNavidrome(_mod.Navidrome):
    def __init__(self, songs):
        super().__init__("http://unused", "u", "p")
        self._songs = songs

    def call(self, method, params=()):
        return {"searchResult3": {"song": self._songs}}


def test_find_track_matches_one_part_of_id3_multi_artist_tag():
    """ID3v2.3 joins artists with ' / '; a match on one of them must count."""
    nav = _FakeNavidrome([{"id": "s1", "title": "STARS", "artist": "¥$ / Kanye West / Ty Dolla $ign"}])
    assert nav.find_track({"title": "STARS", "artists": ["Kanye West", "Ty Dolla $ign"]}) == "s1"


def test_find_track_rejects_other_artist_with_same_title():
    nav = _FakeNavidrome([{"id": "s1", "title": "Stars", "artist": "Grace Potter / The Nocturnals"}])
    assert nav.find_track({"title": "Stars", "artists": ["Kanye West"]}) is None


def test_long_spotify_rate_limit_fails_fast_instead_of_sleeping(monkeypatch):
    """A Retry-After of hours must not hang the fill loop."""
    import email.message
    import urllib.error

    import pytest

    headers = email.message.Message()
    headers["Retry-After"] = "7200"

    def fake_urlopen(req, timeout):
        raise urllib.error.HTTPError(req.full_url, 429, "Too Many Requests", headers, None)

    monkeypatch.setattr(_mod.urllib.request, "urlopen", fake_urlopen)
    monkeypatch.setattr(_mod.time, "sleep", lambda s: pytest.fail(f"slept {s} s"))
    with pytest.raises(RuntimeError, match="rate limit"):
        _mod.http_get("https://api.spotify.com/v1/me")


def test_rate_limit_with_http_date_fails_fast(monkeypatch):
    import email.message
    import urllib.error

    import pytest

    headers = email.message.Message()
    headers["Retry-After"] = "Fri, 16 Oct 2026 23:00:00 GMT"

    def fake_urlopen(req, timeout):
        raise urllib.error.HTTPError(req.full_url, 429, "Too Many Requests", headers, None)

    monkeypatch.setattr(_mod.urllib.request, "urlopen", fake_urlopen)
    monkeypatch.setattr(_mod.time, "sleep", lambda s: pytest.fail(f"slept {s} s"))
    with pytest.raises(RuntimeError, match="rate limit"):
        _mod.http_get("https://api.spotify.com/v1/me")


class _FakeSpotify:
    def __init__(self, playlists, tracks):
        self.playlist_list, self.track_map, self.track_calls = playlists, tracks, []
        self.limited = False

    def playlists(self):
        if self.limited:
            raise _mod.RateLimited("Spotify rate limit: retry after 9999 s")
        return self.playlist_list

    def tracks(self, playlist_id):
        if self.limited:
            raise _mod.RateLimited("Spotify rate limit: retry after 9999 s")
        self.track_calls.append(playlist_id)
        return self.track_map[playlist_id]


def _track(title):
    return {"title": title, "artists": ["A"], "album": "B"}


def test_cache_skips_track_fetch_when_snapshot_is_unchanged(tmp_path):
    fake = _FakeSpotify([{"id": "p1", "name": "P", "snapshot_id": "s1"}], {"p1": [_track("x")]})
    path = tmp_path / "cache.json"
    first = _mod.CachedSpotify(fake, path)
    first.playlists()
    assert first.tracks("p1") == [_track("x")]
    second = _mod.CachedSpotify(fake, path)
    second.playlists()
    assert second.tracks("p1") == [_track("x")]
    assert fake.track_calls == ["p1"]


def test_cache_fetches_again_when_snapshot_changes(tmp_path):
    fake = _FakeSpotify([{"id": "p1", "name": "P", "snapshot_id": "s1"}], {"p1": [_track("x")]})
    path = tmp_path / "cache.json"
    c = _mod.CachedSpotify(fake, path)
    c.playlists()
    c.tracks("p1")
    fake.playlist_list = [{"id": "p1", "name": "P", "snapshot_id": "s2"}]
    fake.track_map = {"p1": [_track("y")]}
    c2 = _mod.CachedSpotify(fake, path)
    c2.playlists()
    assert c2.tracks("p1") == [_track("y")]


def test_cache_is_used_while_spotify_rate_limits(tmp_path):
    fake = _FakeSpotify([{"id": "p1", "name": "P", "snapshot_id": "s1"}], {"p1": [_track("x")]})
    path = tmp_path / "cache.json"
    c = _mod.CachedSpotify(fake, path)
    c.playlists()
    c.tracks("p1")
    fake.limited = True
    c2 = _mod.CachedSpotify(fake, path)
    assert [p["id"] for p in c2.playlists()] == ["p1"]
    assert c2.tracks("p1") == [_track("x")]


def test_rate_limit_without_cache_still_raises(tmp_path):
    import pytest

    fake = _FakeSpotify([], {})
    fake.limited = True
    with pytest.raises(_mod.RateLimited):
        _mod.CachedSpotify(fake, tmp_path / "cache.json").playlists()


def test_lidarr_timeout_skips_the_artist_instead_of_stopping_the_sync(monkeypatch):
    def fake_urlopen(req, timeout):
        raise TimeoutError("timed out")

    monkeypatch.setattr(_mod.urllib.request, "urlopen", fake_urlopen)
    assert _mod.Lidarr("key").add_artist("Kendrick Lamar", 1, 1) is None


def test_find_track_matches_feat_and_bullet_artist_tags():
    nav = _FakeNavidrome([
        {"id": "s1", "title": "Money Trees", "artist": "Kendrick Lamar feat. Jay Rock"},
        {"id": "s2", "title": "Wanna Be Loved", "artist": "Joey Bada$$ • JID • Jo-Vaughn"},
    ])
    assert nav.find_track({"title": "Money Trees", "artists": ["Kendrick Lamar", "Jay Rock"]}) == "s1"
    assert nav.find_track({"title": "Wanna Be Loved", "artists": ["JID"]}) == "s2"


def test_find_track_keeps_artist_names_that_contain_a_comma():
    nav = _FakeNavidrome([{"id": "s1", "title": "EARFQUAKE", "artist": "Tyler, The Creator"}])
    assert nav.find_track({"title": "EARFQUAKE", "artists": ["Tyler, The Creator"]}) == "s1"


def test_accented_artist_names_match_plain_ones():
    assert similar("JAŸ-Z", "Jay-Z") >= MATCH_THRESHOLD
    assert similar("Beyoncé", "Beyonce") >= MATCH_THRESHOLD


def test_find_track_matches_slash_tags_without_spaces():
    nav = _FakeNavidrome([{"id": "s1", "title": "Flashing Lights", "artist": "Kanye West/Dwele"}])
    assert nav.find_track({"title": "Flashing Lights", "artists": ["Kanye West", "Dwele"]}) == "s1"


SPOTDL_REQUIRED = {"name", "artists", "artist", "genres", "disc_number", "disc_count", "album_name",
                   "album_artist", "duration", "year", "date", "track_number", "tracks_count",
                   "song_id", "explicit", "publisher", "url", "isrc", "cover_url", "copyright_text"}

SPOTIFY_TRACK = {
    "id": "t1", "name": "PRIDE.", "duration_ms": 275000, "explicit": True, "disc_number": 1,
    "track_number": 6, "popularity": 70, "external_ids": {"isrc": "USUM71703083"},
    "external_urls": {"spotify": "https://open.spotify.com/track/t1"},
    "artists": [{"name": "Kendrick Lamar", "id": "a1"}],
    "album": {"id": "al1", "name": "DAMN.", "release_date": "2017-04-14", "total_tracks": 14,
              "album_type": "album", "artists": [{"name": "Kendrick Lamar"}],
              "images": [{"url": "https://i.scdn.co/image/x"}]},
}


def test_spotdl_song_has_every_field_spotdl_needs():
    song = _mod.spotdl_song(SPOTIFY_TRACK)
    assert SPOTDL_REQUIRED <= song.keys()
    assert (song["name"], song["album_name"], song["year"], song["duration"]) == ("PRIDE.", "DAMN.", 2017, 275)
    assert song["url"] == "https://open.spotify.com/track/t1"


def test_spotdl_song_accepts_a_year_only_release_date():
    track = {**SPOTIFY_TRACK, "album": {**SPOTIFY_TRACK["album"], "release_date": "1998"}}
    assert _mod.spotdl_song(track)["year"] == 1998


def test_missing_songs_leave_out_tried_and_duplicate_tracks(tmp_path):
    song = _mod.spotdl_song(SPOTIFY_TRACK)
    other = {**song, "url": "https://open.spotify.com/track/t2"}
    missing = [{"song": song}, {"song": song}, {"song": other}, {"title": "local file"}]
    tried = tmp_path / "tried.txt"
    tried.write_text("https://open.spotify.com/track/t2\n")
    out = tmp_path / "missing.spotdl"
    _mod.write_missing_songs(missing, out, tried)
    assert [s["url"] for s in json.loads(out.read_text())] == ["https://open.spotify.com/track/t1"]


def test_rate_limited_playlist_without_cache_is_skipped_not_fatal():
    class LimitedSpotify:
        def tracks(self, playlist_id):
            raise _mod.RateLimited("Spotify rate limit: retry after 9999 s")

    result = _mod.sync_playlist({"id": "p1", "name": "New"}, LimitedSpotify(), None, None,
                                True, {}, 1, 1)
    assert result == {"name": "New", "matched": 0, "total": 0, "missing": [], "skipped": True}


def test_spotdl_song_accepts_a_null_duration():
    assert _mod.spotdl_song({**SPOTIFY_TRACK, "duration_ms": None})["duration"] == 0


def test_playlist_is_saved_newest_added_first_like_spotify_date_added_sort():
    class Spotify:
        def tracks(self, playlist_id):
            return [{**_track("old"), "added_at": "2024-01-01T00:00:00Z"},
                    {**_track("new"), "added_at": "2026-10-01T00:00:00Z"},
                    {**_track("mid"), "added_at": "2025-05-01T00:00:00Z"}]

    class Navidrome:
        saved = None

        def find_track(self, track):
            return track["title"]

        def save_playlist(self, name, song_ids):
            self.saved = song_ids

    navidrome = Navidrome()
    _mod.sync_playlist({"id": "p1", "name": "P"}, Spotify(), navidrome, None, False, {}, 1, 1)
    assert navidrome.saved == ["new", "mid", "old"]

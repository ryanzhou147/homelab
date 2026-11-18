//! Finds single songs on Soulseek through slskd, downloads the best match, and files it into the
//! music library under "Soulseek singles", where Navidrome finds it. Lidarr works by album; this
//! is for songs that come without an album, such as a pasted playlist link.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OpusFile, VorbisFile};
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::common::media_dir;
use crate::navidrome::agent;

const API: &str = "http://127.0.0.1:5030/slskd/api/v0";
const AUDIO: [&str; 5] = ["flac", "mp3", "m4a", "ogg", "opus"];
const MIN_MP3_KBPS: u64 = 192;
const MAX_LENGTH_GAP_S: u64 = 15;
const ONLINE_GAP_S: u64 = 5;
// Searches are spaced out so the Soulseek server does not throttle the account.
const SEARCH_GAP: Duration = Duration::from_secs(8);
const SEARCH_WAIT: Duration = Duration::from_secs(45);
const RETRY_GAP: Duration = Duration::from_secs(60);
const DOWNLOAD_WAIT: Duration = Duration::from_secs(30 * 60);
const NOT_RUNNING: &str = "slskd is not running";
// The playlist is saved again after this many new songs, or after this long with any.
const UPDATE_EVERY: usize = 5;
const UPDATE_GAP: Duration = Duration::from_secs(5 * 60);
static DIGITS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\d+").unwrap());
static NOT_WORDS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\(.*?\)|\[.*?\]|\b(?:feat|ft)\..*|\s-\s.*|\S*\*\S*").unwrap()
});
static NOT_SEARCHABLE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[^\w\s']").unwrap());

/// A song to look for: title, artists and length in seconds (0 when unknown).
pub struct Wanted<'a> {
    pub title: &'a str,
    pub artists: &'a [String],
    pub seconds: u64,
}

/// Lowercase words of two characters or more, accents removed (other scripts stay), without
/// (..), [..], "feat." parts or words with a '*' in them (a censored "Sh*t" never matches the
/// uncensored file name).
fn words(s: &str) -> Vec<String> {
    tokens(&NOT_WORDS.replace_all(s, " "))
}

/// Every lowercase word of two characters or more, accents removed. For a peer's file path,
/// where removing parts could only remove the title.
fn tokens(s: &str) -> Vec<String> {
    let folded: String = s
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 1)
        .map(String::from)
        .collect()
}

fn numbers(s: &str) -> Vec<String> {
    DIGITS
        .find_iter(s)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Whether a named recording from an online search is the song: every title word and number in
/// its name, one artist word in its people, no version word the title lacks, and a length within
/// ONLINE_GAP_S. Stricter than a Soulseek file, because uploads include edits and covers.
pub fn is_song(song: &Wanted, name: &str, who: &str, seconds: u64) -> bool {
    let have: std::collections::HashSet<String> = tokens(name).into_iter().collect();
    let people: std::collections::HashSet<String> =
        tokens(&format!("{who} {name}")).into_iter().collect();
    let title = words(song.title);
    // Words that name the artist: not "the" or "and", and not title words, which every cover
    // has ("The Ink Spots" and "I Don't Want to Set the World on Fire").
    let all: Vec<String> = song.artists.iter().flat_map(|a| words(a)).collect();
    let named: Vec<String> = (all.iter())
        .filter(|w| !["the", "and"].contains(&w.as_str()) && !title.contains(w))
        .cloned()
        .collect();
    let artist = if named.is_empty() { all } else { named };
    let in_name = numbers(name);
    !title.is_empty()
        && title.iter().all(|w| have.contains(w))
        && (artist.is_empty() || artist.iter().any(|w| people.contains(w)))
        && (numbers(&NOT_WORDS.replace_all(song.title, "")))
            .iter()
            .all(|n| in_name.contains(n))
        && !crate::text::other_version(name, song.title)
        && (song.seconds == 0 || (seconds > 0 && song.seconds.abs_diff(seconds) <= ONLINE_GAP_S))
}

/// Higher is better; None means the file is not this song.
fn score(song: &Wanted, file: &Value) -> Option<f64> {
    let name = file["filename"].as_str()?.replace('\\', "/");
    let base = name.rsplit('/').next().unwrap_or(&name);
    let ext = base.rsplit_once('.').map(|(_, e)| e.to_lowercase())?;
    if !AUDIO.contains(&ext.as_str()) {
        return None;
    }
    let path_words: std::collections::HashSet<String> = tokens(&name).into_iter().collect();
    let title = words(song.title);
    if title.is_empty() || !title.iter().all(|w| path_words.contains(w)) {
        return None;
    }
    let artist: Vec<String> = song.artists.iter().flat_map(|a| words(a)).collect();
    if !artist.is_empty() && !artist.iter().any(|w| path_words.contains(w)) {
        return None;
    }
    // "Butcher Pete Pt. 1" is not Pt. 2: every number of the title must be in the name too.
    // The file name only: a "Disc 2" folder says nothing about the part.
    // shortcut: a leading track number ("2 - Butcher Pete Pt. 1.flac") still passes for Pt. 2;
    // stripping it would reject "99 Problems.flac". Upgrade if a wrong part shows up.
    let have = numbers(base);
    if !numbers(&NOT_WORDS.replace_all(song.title, ""))
        .iter()
        .all(|n| have.contains(n))
    {
        return None;
    }
    // "Atom Bomb Baby - Instrumental.flac" is not the song asked for, whatever its quality.
    if crate::text::other_version(base, song.title) {
        return None;
    }
    let kbps = file["bitRate"].as_u64().unwrap_or(0);
    if ext == "mp3" && kbps < MIN_MP3_KBPS {
        return None;
    }
    let have = file["length"].as_u64().unwrap_or(0);
    if song.seconds > 0 && have > 0 && song.seconds.abs_diff(have) > MAX_LENGTH_GAP_S {
        return None;
    }
    let quality = if ext == "flac" {
        3.0
    } else if kbps >= 320 {
        2.0
    } else {
        1.0
    };
    let extra = tokens(base)
        .iter()
        .filter(|w| !title.contains(w) && !artist.contains(w))
        .count();
    Some(quality - extra as f64 * 0.05)
}

/// The best file among the users with a free upload slot: (user, file).
fn best_match(song: &Wanted, responses: &[Value]) -> Option<(String, Value)> {
    let mut picks: Vec<(f64, u64, String, Value)> = Vec::new();
    for r in responses.iter().filter(|r| r["hasFreeUploadSlot"] == true) {
        for f in r["files"].as_array().into_iter().flatten() {
            if let Some(s) = score(song, f) {
                let user = r["username"].as_str().unwrap_or_default().to_string();
                picks.push((s, r["uploadSpeed"].as_u64().unwrap_or(0), user, f.clone()));
            }
        }
    }
    picks.sort_by(|a, b| {
        (b.0, b.1)
            .partial_cmp(&(a.0, a.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    picks.into_iter().next().map(|(_, _, u, f)| (u, f))
}

fn call(method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let req = ureq::http::Request::builder()
        .method(method)
        .uri(format!("{API}{path}"))
        .header("Content-Type", "application/json")
        .body(body.map(|b| b.to_string()).unwrap_or_default())?;
    let mut resp = agent()
        .run(req)
        .with_context(|| format!("slskd {method} {path}"))?;
    let status = resp.status();
    let text = resp.body_mut().read_to_string()?;
    // The fetch gate answers 502 when slskd behind it is down.
    if status == 502 {
        bail!(
            "{NOT_RUNNING}; the disk guard stops it below 20 GB free (start it with: docker compose start slskd soularr)"
        );
    }
    if !status.is_success() {
        bail!("slskd {method} {path}: HTTP {status}");
    }
    Ok(if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text)?
    })
}

/// One search, and one more after a pause when it gets no answers: the Soulseek server sometimes
/// answers nothing for a short time when many searches come close together.
fn search(text: &str, log: &mut dyn FnMut(String)) -> Result<Vec<Value>> {
    let found = search_once(text)?;
    if !found.is_empty() {
        return Ok(found);
    }
    log(format!(
        "no answers for {text}, trying again in {} s",
        RETRY_GAP.as_secs()
    ));
    sleep(RETRY_GAP);
    search_once(text)
}

fn search_once(text: &str) -> Result<Vec<Value>> {
    let id = call("POST", "/searches", Some(json!({"searchText": text})))?["id"]
        .as_str()
        .context("slskd gave no search id")?
        .to_string();
    let start = Instant::now();
    while start.elapsed() < SEARCH_WAIT {
        sleep(Duration::from_secs(3));
        if call("GET", &format!("/searches/{id}"), None)?["isComplete"] == true {
            break;
        }
    }
    let responses = call("GET", &format!("/searches/{id}/responses"), None)?;
    let _ = call("DELETE", &format!("/searches/{id}"), None);
    Ok(responses.as_array().cloned().unwrap_or_default())
}

/// Where slskd puts a remote file: its last folder and its name under the download folder.
/// Empty, "." and ".." parts are left out, so a peer's name cannot point elsewhere.
fn local_path(downloads: &Path, remote: &str) -> PathBuf {
    let parts: Vec<&str> = remote
        .split(['\\', '/'])
        .filter(|p| !matches!(*p, "" | "." | ".."))
        .collect();
    let folder = if parts.len() > 1 {
        parts[parts.len() - 2]
    } else {
        ""
    };
    downloads.join(folder).join(parts.last().unwrap_or(&""))
}

pub fn safe(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    if t.trim().is_empty() {
        "Unknown".into()
    } else {
        t.trim().to_string()
    }
}

/// What to ask Soulseek: the main artist and the title without Spotify's "(with ...)",
/// "[...]" or "feat." parts, which files rarely carry and which make the search find nothing.
fn search_text(artist: &str, title: &str) -> String {
    let title = NOT_WORDS.replace_all(title, " ");
    let text = NOT_SEARCHABLE
        .replace_all(&format!("{artist} {title}"), " ")
        .into_owned();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether to save the playlist again: some new songs, and enough of them, enough time, or the end.
fn due(new_songs: usize, since_last: Duration, end: bool) -> bool {
    new_songs > 0 && (end || new_songs >= UPDATE_EVERY || since_last >= UPDATE_GAP)
}

/// Runs when new songs are in the library; it gets the job's log.
/// It returns false when the update failed, so the next one tries again.
pub type OnFiled<'a> = dyn FnMut(&mut dyn FnMut(String)) -> bool + 'a;

/// Sets the title and artist to the requested song's. Peers' tags are often wrong or empty
/// ("Time Life", "Various Artists"), and Navidrome matches the playlist by these tags.
/// FLAC, OGG and Opus go through their own types: lofty's generic tag drops every Vorbis key it
/// does not know ("ALBUM ARTIST", "TOTALTRACKS", MusicBrainz keys) when it saves.
pub fn write_tags(path: &Path, song: &Wanted) -> Result<()> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    let open = || -> Result<std::fs::File> { Ok(std::fs::File::open(path)?) };
    let save = WriteOptions::default();
    match ext.as_deref() {
        Some("flac") => {
            let mut f = FlacFile::read_from(&mut open()?, ParseOptions::new())?;
            if f.vorbis_comments().is_none() {
                f.set_vorbis_comments(VorbisComments::default());
            }
            set_comments(f.vorbis_comments_mut().context("no comments")?, song);
            f.save_to_path(path, save)?;
        }
        Some("ogg") => {
            let mut f = VorbisFile::read_from(&mut open()?, ParseOptions::new())?;
            set_comments(f.vorbis_comments_mut(), song);
            f.save_to_path(path, save)?;
        }
        Some("opus") => {
            let mut f = OpusFile::read_from(&mut open()?, ParseOptions::new())?;
            set_comments(f.vorbis_comments_mut(), song);
            f.save_to_path(path, save)?;
        }
        _ => {
            let mut file = lofty::read_from_path(path)?;
            if file.primary_tag().is_none() {
                file.insert_tag(Tag::new(file.primary_tag_type()));
            }
            let tag = file.primary_tag_mut().context("no tag")?;
            tag.set_title(song.title.to_string());
            tag.set_artist(song.artists.join("; "));
            tag.remove_key(ItemKey::TrackArtists);
            tag.save_to_path(path, save)?;
        }
    }
    Ok(())
}

fn set_comments(comments: &mut VorbisComments, song: &Wanted) {
    comments.insert("TITLE".into(), song.title.to_string());
    comments.insert("ARTIST".into(), song.artists.join("; "));
    // Navidrome takes ARTISTS over ARTIST, so a peer's stale one would still name the artist.
    comments.remove("ARTISTS").for_each(drop);
}

/// Moves the finished downloads among `queued` into the library; returns how many it moved.
/// slskd keeps unfinished files in its own incomplete folder, so a file in `downloads` is done.
fn file_finished(queued: &mut Vec<(&Wanted, String)>, log: &mut dyn FnMut(String)) -> usize {
    let downloads = media_dir().join("downloads/slskd");
    let library = media_dir().join("music/Soulseek singles");
    let mut filed = 0;
    // Tagged again too when already filed: an earlier job may have filed it with a peer's tags.
    let tag = |path: &Path, song: &Wanted, log: &mut dyn FnMut(String)| {
        if let Err(e) = write_tags(path, song) {
            log(format!("could not tag {}: {e:#}", path.display()));
        }
    };
    queued.retain(|(song, remote)| {
        let artist = song.artists.first().map_or("", String::as_str);
        let src = local_path(&downloads, remote);
        if !src.exists() {
            // An earlier job filed it (a 409 for a file that is done): nothing to wait for.
            let done = src.file_name().map(|n| library.join(safe(artist)).join(n));
            if let Some(done) = done.filter(|d| d.exists()) {
                log(format!("already in the library: {}", src.display()));
                tag(&done, song, log);
                filed += 1;
                return false;
            }
            return true;
        }
        // shortcut: a Soularr download with the same folder and file name would be taken too,
        // and two songs whose peer files share a name ("01.flac") look filed, so the retag can
        // mislabel one; rare, so not tracked by transfer id. Upgrade: name files after the title.
        let dest = library.join(safe(artist));
        let name = src.file_name().unwrap_or_default().to_owned();
        if dest.join(&name).exists() {
            log(format!(
                "already in the library: {}",
                name.to_string_lossy()
            ));
            tag(&dest.join(&name), song, log);
            filed += 1;
            return false;
        }
        // Tagged before the move, so a write that fails halfway never harms a library file.
        tag(&src, song, log);
        match std::fs::create_dir_all(&dest).and_then(|()| std::fs::rename(&src, dest.join(&name)))
        {
            Ok(()) => {
                filed += 1;
                log(format!("filed: {}", name.to_string_lossy()));
            }
            Err(e) => log(format!("could not file {}: {e}", src.display())),
        }
        false
    });
    filed
}

/// 1 when the online sources had the song and it is now filed, else 0 (with a log line).
fn online_get(song: &Wanted, text: &str, log: &mut dyn FnMut(String)) -> usize {
    match crate::online::get(song, text, log) {
        Ok(true) => 1,
        Ok(false) => {
            log(format!("not found online either: {text}"));
            0
        }
        Err(e) => {
            log(format!("online download failed for {text}: {e:#}"));
            0
        }
    }
}

/// Searches, downloads and files each song; returns how many were filed. Songs go into the
/// library as their downloads finish, and `on_filed` runs after every few of them (and at the
/// end), so the playlist fills while the search goes on. `log` gets one line per song. A failed
/// search or download skips that song; a stopped slskd stops the job.
pub fn fetch(
    songs: &[Wanted],
    log: &mut dyn FnMut(String),
    on_filed: &mut OnFiled,
) -> Result<usize> {
    let mut queued = Vec::new();
    let (mut filed, mut unannounced, mut last_update) = (0, 0, Instant::now());
    crate::online::update(log);
    // `online`: songs filed from online searches since the last call.
    let mut collect = |queued: &mut Vec<(&Wanted, String)>,
                       log: &mut dyn FnMut(String),
                       force: bool,
                       online: usize| {
        let new = file_finished(queued, log) + online;
        filed += new;
        unannounced += new;
        if due(unannounced, last_update.elapsed(), force) {
            // After a failure keep one, so the end of the job tries again; the next try waits
            // for a few more songs or the time gap instead of following every song.
            unannounced = if on_filed(log) { 0 } else { 1 };
            last_update = Instant::now();
        }
    };
    let mut online = 0;
    for (i, song) in songs.iter().enumerate() {
        let artist = song.artists.first().map_or("", String::as_str);
        let text = search_text(artist, song.title);
        let n = format!("[{}/{}]", i + 1, songs.len());
        match search(&text, log).map(|r| best_match(song, &r)) {
            Ok(Some((user, file))) => {
                let body = json!([{"filename": file["filename"], "size": file["size"]}]);
                let path = format!("/transfers/downloads/{}", crate::text::quote(&user, ""));
                let remote = file["filename"].as_str().unwrap_or("").to_string();
                match call("POST", &path, Some(body)) {
                    Ok(_) => {
                        log(format!("{n} downloading: {text}"));
                        queued.push((song, remote));
                    }
                    // An earlier job queued the same file; it still arrives, so wait for it.
                    Err(e) if e.to_string().contains("HTTP 409") => {
                        log(format!("{n} already downloading: {text}"));
                        queued.push((song, remote));
                    }
                    Err(e) => {
                        log(format!("{n} could not queue {text}: {e}"));
                        online = online_get(song, &text, log);
                    }
                }
            }
            Ok(None) => {
                log(format!("{n} not found on Soulseek: {text}"));
                online = online_get(song, &text, log);
            }
            Err(e) if e.to_string().starts_with(NOT_RUNNING) => {
                collect(&mut queued, log, true, 0);
                return Err(e);
            }
            Err(e) => {
                log(format!("{n} search failed for {text}: {e}"));
                online = online_get(song, &text, log);
            }
        }
        sleep(SEARCH_GAP);
        collect(&mut queued, log, false, std::mem::take(&mut online));
    }
    let start = Instant::now();
    while !queued.is_empty() && start.elapsed() < DOWNLOAD_WAIT {
        sleep(Duration::from_secs(20));
        collect(&mut queued, log, false, 0);
    }
    collect(&mut queued, log, false, 0);
    for (song, remote) in &queued {
        log(format!(
            "still downloading after 30 min, left out: {remote}"
        ));
        let artist = song.artists.first().map_or("", String::as_str);
        online += online_get(song, &search_text(artist, song.title), log);
    }
    // shortcut: a left-out Soulseek file that arrives later stays in the downloads folder.
    queued.clear();
    collect(&mut queued, log, true, online);
    Ok(filed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_tags_keeps_flac_keys_lofty_does_not_know() {
        // A FLAC with only STREAMINFO (44.1 kHz, stereo, 16 bit) and a peer's comments.
        let mut info = vec![0x10, 0, 0x10, 0, 0, 0, 0, 0, 0, 0];
        info.extend((44100u64 << 44 | 1 << 41 | 15 << 36).to_be_bytes());
        info.extend([0; 16]);
        let flac = [b"fLaC".as_slice(), &[0x80, 0, 0, 34], &info].concat();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Rocket 69.flac");
        std::fs::write(&path, flac).unwrap();
        let mut f = FlacFile::read_from(
            &mut std::fs::File::open(&path).unwrap(),
            ParseOptions::new(),
        )
        .unwrap();
        let mut peer = VorbisComments::default();
        peer.insert("ALBUM ARTIST".into(), "Todd Rhodes".into());
        peer.insert("ARTISTS".into(), "Connie Allen".into());
        f.set_vorbis_comments(peer);
        f.save_to_path(&path, WriteOptions::default()).unwrap();

        let artists = ["Todd Rhodes".to_string(), "Connie Allen".to_string()];
        write_tags(
            &path,
            &Wanted {
                title: "Rocket 69",
                artists: &artists,
                seconds: 0,
            },
        )
        .unwrap();
        let f = FlacFile::read_from(
            &mut std::fs::File::open(&path).unwrap(),
            ParseOptions::new(),
        )
        .unwrap();
        let c = f.vorbis_comments().unwrap();
        assert_eq!(c.get("ALBUM ARTIST"), Some("Todd Rhodes"));
        assert_eq!(c.get("ARTIST"), Some("Todd Rhodes; Connie Allen"));
        assert_eq!(c.get("TITLE"), Some("Rocket 69"));
        assert_eq!(c.get("ARTISTS"), None);
    }

    #[test]
    fn write_tags_sets_an_untagged_mp3s_artist() {
        use crate::merge_tags::tests::{mp3, talb};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Jingle Jangle Jingle.mp3");
        mp3(&path, vec![talb("Time Life")]);
        let artists = ["Kay Kyser & His Orchestra".to_string()];
        let wanted = Wanted {
            title: "(I Got Spurs That) Jingle Jangle Jingle",
            artists: &artists,
            seconds: 0,
        };
        write_tags(&path, &wanted).unwrap();
        let file = lofty::read_from_path(&path).unwrap();
        let tag = file.primary_tag().unwrap();
        assert_eq!(tag.artist().as_deref(), Some("Kay Kyser & His Orchestra"));
        assert_eq!(tag.title().as_deref(), Some(wanted.title));
        assert_eq!(tag.album().as_deref(), Some("Time Life"));
    }

    fn song<'a>(artists: &'a [String]) -> Wanted<'a> {
        Wanted {
            title: "Third Eye Sh*t",
            artists,
            seconds: 200,
        }
    }

    #[test]
    fn scores_like_the_python_check() {
        let artists = vec!["Joey Bada$$".to_string()];
        let s = song(&artists);
        let good = json!({"filename": r"Music\Joey Bada$$ - 1999\05 Third Eye Shit.mp3", "bitRate": 320, "length": 201});
        let low = json!({"filename": r"Music\Joey Bada$$ - 1999\05 Third Eye Shit.mp3", "bitRate": 128, "length": 201});
        let wrong = json!({"filename": r"Music\Joey Bada$$\Christ Conscious.mp3", "bitRate": 320, "length": 200});
        let flac =
            json!({"filename": r"Music\Joey Bada$$ - 1999\05 Third Eye Shit.flac", "length": 201});
        let long = json!({"filename": r"Music\Joey Bada$$ - 1999\05 Third Eye Shit.mp3", "bitRate": 320, "length": 400});
        let pt = Wanted {
            title: "Butcher Pete, Pt. 2",
            artists: &artists,
            seconds: 0,
        };
        let pt1 = json!({"filename": r"Music\Joey Bada$$\Butcher Pete Pt. 1.flac"});
        let pt2 = json!({"filename": r"Music\Joey Bada$$\03 Butcher Pete Pt. 2.flac"});
        assert!(score(&pt, &pt1).is_none() && score(&pt, &pt2).is_some());
        let karaoke = json!({"filename": r"Music\Joey Bada$$\Third Eye Shit (Instrumental).flac", "length": 201});
        assert!(score(&s, &karaoke).is_none());
        assert!(score(&s, &good).is_some());
        assert!(
            score(&s, &low).is_none() && score(&s, &wrong).is_none() && score(&s, &long).is_none()
        );
        assert!(score(&s, &flac).unwrap() > score(&s, &good).unwrap());
        let resp = vec![
            json!({"username": "a", "hasFreeUploadSlot": true, "uploadSpeed": 1, "files": [good]}),
            json!({"username": "b", "hasFreeUploadSlot": false, "uploadSpeed": 9, "files": [flac]}),
        ];
        assert_eq!(best_match(&s, &resp).unwrap().0, "a");
    }

    #[test]
    fn matches_titles_in_other_scripts() {
        let artists = vec!["YOASOBI".to_string()];
        let s = Wanted {
            title: "夜に駆ける",
            artists: &artists,
            seconds: 0,
        };
        let file = json!({"filename": r"Music\YOASOBI\01 夜に駆ける.flac"});
        assert!(score(&s, &file).is_some());
        assert_eq!(words("Beyoncé (Live)"), ["beyonce"]);
        let gift = Wanted {
            title: "Gift",
            artists: &artists,
            seconds: 0,
        };
        assert!(score(&gift, &json!({"filename": r"Music\YOASOBI\05 Gift.flac"})).is_some());
        let take_care = Wanted {
            title: "Take Care",
            artists: &artists,
            seconds: 0,
        };
        let feat =
            json!({"filename": r"Music\YOASOBI feat. Rihanna - Take Care.mp3", "bitRate": 320});
        assert!(score(&take_care, &feat).is_some());
        assert_eq!(words("Defeat (feat. X)"), ["defeat"]);
    }

    #[test]
    fn searches_for_the_main_artist_and_title() {
        let text = search_text("The Beatles", "Hey Jude - Remastered 2015");
        assert_eq!(text, "The Beatles Hey Jude");
        let text = search_text("JID", "Wholeheartedly (with Ty Dolla $ign & 6LACK)");
        assert_eq!(text, "JID Wholeheartedly");
        assert_eq!(
            search_text("Alesso", "Heroes (we could be)"),
            "Alesso Heroes"
        );
        assert_eq!(search_text("AC/DC", "T.N.T. - Remastered"), "AC DC T N T");
    }

    #[test]
    fn updates_the_playlist_every_few_songs_or_minutes() {
        let minute = Duration::from_secs(60);
        assert!(!due(0, 10 * minute, true));
        assert!(!due(2, minute, false));
        assert!(due(2, minute, true) && due(5, minute, false) && due(1, 5 * minute, false));
    }

    #[test]
    fn maps_remote_paths_and_names() {
        let p = local_path(Path::new("/d"), r"@@x\Music\Album\01 Song.flac");
        assert_eq!(p, Path::new("/d/Album/01 Song.flac"));
        assert_eq!(
            local_path(Path::new("/d"), r"x\..\Song.mp3"),
            Path::new("/d/x/Song.mp3")
        );
        assert_eq!(safe("AC/DC: Live?"), "AC_DC_ Live_");
        assert_eq!(safe("  "), "Unknown");
    }
}

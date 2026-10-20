//! Merge albums that Navidrome shows twice because their files have different album tags.
//!
//! Lidarr files have a MusicBrainz album ID and spotdl files do not, so one album folder can show
//! as two albums. For each album that Navidrome lists more than once with the same album artist,
//! name and year, this keeps the copy with a MusicBrainz ID (else the one with the most songs).
//! Songs that copy already has move to $MEDIA/music-duplicates; the other files get its album
//! tags, and Navidrome merges them at its next scan. Old tags and moves go to
//! $DATA/merge-split-albums.jsonl. Without --apply it only prints the plan.

use crate::common::{data_dir, home};
use crate::merge_tags::{Tags, album_tags, retag};
use anyhow::{Context, Result};
use regex::Regex;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(clap::Args)]
pub struct Args {
    /// Move and retag the files; without it, only print the plan.
    #[arg(long)]
    apply: bool,
}

/// Two files hold the same song when their base titles match and so do the disc and track
/// numbers. When the title occurs once on each side, lengths that differ by less than this also
/// count: spotdl's YouTube copies run a few seconds longer and can have other track numbers.
const SAME_SONG_SECONDS: f64 = 8.0;
const VERSION_WORDS: [&str; 5] = ["remix", "live", "acoustic", "instrumental", "demo"];

/// path, title, disc, track, seconds
type Song = (String, String, Option<i64>, Option<i64>, f64);
/// disc, track, seconds of a song the kept copy has
type Seen = (Option<i64>, Option<i64>, f64);
/// A file of the kept copy, files to retag, duplicate files.
type Merge = (String, Vec<String>, Vec<String>);

struct Paths {
    music: PathBuf,
    duplicates: PathBuf,
    navidrome_db: PathBuf,
    undo_log: PathBuf,
}

impl Paths {
    fn from_env() -> Self {
        let data = data_dir();
        let media = std::env::var_os("MEDIA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join("homelab-media"));
        Paths {
            music: media.join("music"),
            duplicates: media.join("music-duplicates"),
            navidrome_db: data.join("navidrome/navidrome.db"),
            undo_log: data.join("merge-split-albums.jsonl"),
        }
    }
}

/// 'Payday (feat. Young Thug)' -> 'payday': the title before any (, [ or ' - ' tag.
fn base_title(title: &str) -> String {
    static SPLIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[(\[]| - ").expect("valid regex"));
    static WORDS: LazyLock<Vec<(&str, Regex)>> = LazyLock::new(|| {
        let re = |w: &str| Regex::new(&format!(r"\b{w}\b")).expect("valid regex");
        VERSION_WORDS.iter().map(|&w| (w, re(w))).collect()
    });
    let lower = title.to_lowercase();
    let head = SPLIT.split(&lower).next().unwrap_or_default();
    let mut head: String = head
        .chars()
        .filter(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
        .collect();
    if head.is_empty() {
        head.clone_from(&lower);
    }
    // A remix or live take of a song is another song.
    for (word, re) in WORDS.iter() {
        if re.is_match(&lower) {
            head.push_str(word);
        }
    }
    head
}

/// Split the other copies' files into (files to retag, songs the album already has).
fn sort_copies(kept: &[Song], others: &[Song]) -> (Vec<String>, Vec<String>) {
    let count = |songs: &[Song]| {
        let mut n: HashMap<String, usize> = HashMap::new();
        for s in songs {
            *n.entry(base_title(&s.1)).or_default() += 1;
        }
        n
    };
    let (kept_count, other_count) = (count(kept), count(others));
    let mut seen: HashMap<String, Vec<Seen>> = HashMap::new();
    for (_, title, disc, track, seconds) in kept {
        seen.entry(base_title(title))
            .or_default()
            .push((*disc, *track, *seconds));
    }
    let (mut to_retag, mut duplicates) = (vec![], vec![]);
    for (path, title, disc, track, seconds) in others {
        let base = base_title(title);
        let by_length = kept_count.get(&base).copied().unwrap_or(0) <= 1 && other_count[&base] == 1;
        let matches = seen.entry(base).or_default();
        let same = |&(d, t, s): &Seen| {
            (d, t) == (*disc, *track) || (by_length && (s - seconds).abs() < SAME_SONG_SECONDS)
        };
        if matches.iter().any(same) {
            duplicates.push(path.clone());
        } else {
            to_retag.push(path.clone());
            matches.push((*disc, *track, *seconds));
        }
    }
    (to_retag, duplicates)
}

fn songs(db: &Connection, album_id: &str) -> Result<Vec<Song>> {
    let mut q = db.prepare(
        "select path, title, max(disc_number, 1), track_number, duration
        from media_file where album_id = ? and missing = 0 order by path",
    )?;
    let rows = q.query_map([album_id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Return (a file of the kept copy, files to retag, duplicate files) for each split album.
///
/// The other copies of an album name join the kept copy when they have the same year, or share
/// a song with it (a reissue). A copy with another MusicBrainz release ID, such as a single,
/// stays.
fn split_albums(db: &Connection) -> Result<Vec<Merge>> {
    let mut q = db.prepare(
        "select lower(album_artist), lower(name) from album where missing = 0
        group by 1, 2 having count(*) > 1",
    )?;
    let groups = q.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<String>>(1)?,
        ))
    })?;
    let mut copies = db.prepare(
        "select id, coalesce(mbz_album_id, ''), max_year from album
        where lower(album_artist) = ? and lower(name) = ? and missing = 0
        order by coalesce(mbz_album_id, '') = '', song_count desc, id",
    )?;
    let mut plan = vec![];
    for key in groups {
        let key = key?;
        let albums = copies
            .query_map(rusqlite::params![key.0, key.1], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let ((kept_id, kept_mbid, kept_year), rest) =
            albums.split_first().context("album group has no albums")?;
        let kept = songs(db, kept_id)?;
        let mut others = vec![];
        for (album_id, mbid, year) in rest {
            let songs = songs(db, album_id)?;
            if !mbid.is_empty() && mbid != kept_mbid {
                continue;
            }
            if year == kept_year || !sort_copies(&kept, &songs).1.is_empty() {
                others.extend(songs);
            }
        }
        let (to_retag, duplicates) = sort_copies(&kept, &others);
        if !kept.is_empty() && !others.is_empty() {
            plan.push((kept[0].0.clone(), to_retag, duplicates));
        }
    }
    Ok(plan)
}

/// A JSON string as Python's json.dumps writes it: ASCII only, other characters as \uXXXX.
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// An undo-log line for a retag, in json.dumps form: {"path": ..., "old": {"album": [...]}}.
fn retag_line(rel: &str, old: &Tags) -> String {
    let old: Vec<String> = old
        .iter()
        .map(|(k, values)| {
            let values: Vec<String> = values.iter().map(|v| json_str(v)).collect();
            format!("{}: [{}]", json_str(k), values.join(", "))
        })
        .collect();
    format!(
        "{{\"path\": {}, \"old\": {{{}}}}}",
        json_str(rel),
        old.join(", ")
    )
}

/// Path(rel).parent in Python: "." for a file at the top.
fn parent(rel: &str) -> String {
    match Path::new(rel).parent() {
        Some(p) if !p.as_os_str().is_empty() => p.display().to_string(),
        _ => ".".into(),
    }
}

/// Move the duplicates, then retag the rest; return (files retagged, duplicates moved).
fn apply_merge(
    paths: &Paths,
    undo: &mut File,
    merge: &Merge,
    new: &Tags,
) -> Result<(usize, usize)> {
    let (_, to_retag, duplicates) = merge;
    let (mut retagged, mut moved) = (0, 0);
    for rel in duplicates {
        let target = paths.duplicates.join(rel);
        if target.exists() {
            eprintln!("  skipped {rel}: {} exists", target.display());
            continue;
        }
        let moving = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(paths.music.join(rel), &target));
        if let Err(e) = moving {
            eprintln!("  skipped {rel}: {e}");
            continue;
        }
        let target = json_str(&target.display().to_string());
        writeln!(
            undo,
            "{{\"path\": {}, \"moved_to\": {target}}}",
            json_str(rel)
        )?;
        moved += 1;
    }
    for rel in to_retag {
        match retag(&paths.music.join(rel), new) {
            Ok(old) => writeln!(undo, "{}", retag_line(rel, &old))?,
            Err(e) => {
                eprintln!("  skipped {rel}: {e:#}");
                continue;
            }
        }
        retagged += 1;
    }
    Ok((retagged, moved))
}

fn merge(paths: &Paths, apply: bool) -> Result<()> {
    let db = Connection::open_with_flags(&paths.navidrome_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let plan = split_albums(&db)?;
    let (mut retagged, mut moved) = (0, 0);
    // The Python opens the log before the loop, so --apply creates it even with nothing to log.
    let mut undo = if apply {
        Some(
            OpenOptions::new()
                .append(true)
                .create(true)
                .open(&paths.undo_log)?,
        )
    } else {
        None
    };
    for merge in &plan {
        let (keep, to_retag, duplicates) = merge;
        let new = match album_tags(&paths.music.join(keep)) {
            Ok(new) => new,
            Err(e) => {
                eprintln!("  skipped {}: {e:#}", parent(keep));
                continue;
            }
        };
        if !new.iter().any(|(k, _)| k == "album") {
            eprintln!("  skipped {}: {keep} has no album tag", parent(keep));
            continue;
        }
        println!(
            "{}: retag {}, move {}",
            parent(keep),
            to_retag.len(),
            duplicates.len()
        );
        if let Some(undo) = undo.as_mut() {
            let (r, m) = apply_merge(paths, undo, merge, &new)?;
            retagged += r;
            moved += m;
        }
    }
    let dry = if apply { "" } else { " (dry run)" };
    println!(
        "{} split albums: {retagged} files retagged, {moved} duplicates moved{dry}",
        plan.len()
    );
    Ok(())
}

pub fn run(args: Args) -> Result<()> {
    merge(&Paths::from_env(), args.apply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge_tags::tests::{mp3, talb, txxx};

    type AlbumRow<'a> = (&'a str, &'a str, &'a str, i64, &'a str, i64);
    type FileRow<'a> = (&'a str, &'a str, &'a str, i64, i64);

    /// albums: (id, artist, name, year, mbid, songs); files: (path, title, album, track, seconds).
    fn db(albums: &[AlbumRow], files: &[FileRow], path: Option<&Path>) -> Connection {
        let db = match path {
            Some(p) => Connection::open(p).unwrap(),
            None => Connection::open_in_memory().unwrap(),
        };
        db.execute_batch(
            "create table album (id, album_artist, name, max_year, mbz_album_id, song_count, missing);
            create table media_file (path, title, album_id, disc_number, track_number, duration, missing);",
        )
        .unwrap();
        for a in albums {
            db.execute(
                "insert into album values (?, ?, ?, ?, ?, ?, 0)",
                rusqlite::params![a.0, a.1, a.2, a.3, a.4, a.5],
            )
            .unwrap();
        }
        for f in files {
            db.execute(
                "insert into media_file values (?, ?, ?, 0, ?, ?, 0)",
                rusqlite::params![f.0, f.1, f.2, f.3, f.4],
            )
            .unwrap();
        }
        db
    }

    fn merge(keep: &str, to_retag: &[&str], duplicates: &[&str]) -> Merge {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        (keep.into(), own(to_retag), own(duplicates))
    }

    fn plan(albums: &[AlbumRow], files: &[FileRow]) -> Vec<Merge> {
        split_albums(&db(albums, files, None)).unwrap()
    }

    #[test]
    fn tagged_copy_is_kept_and_its_songs_are_duplicates() {
        // The Lidarr copy stays even when smaller; spotdl songs it has are duplicates.
        let p = plan(
            &[
                ("a", "Nas", "Hip Hop Is Dead", 2006, "", 3),
                ("b", "nas", "hip hop is dead", 2006, "mb1", 2),
            ],
            &[
                ("s/1.mp3", "Intro", "a", 1, 100),
                ("s/2.mp3", "Carry On", "a", 3, 200),
                ("s/3.mp3", "Hope", "a", 4, 150),
                ("l/1.flac", "intro", "b", 1, 98),
                ("l/2.flac", "Can't Forget", "b", 2, 210),
            ],
        );
        assert_eq!(
            p,
            [merge("l/1.flac", &["s/2.mp3", "s/3.mp3"], &["s/1.mp3"])]
        );
    }

    #[test]
    fn same_track_and_length_is_a_duplicate_even_with_another_title() {
        // 'Payday' and 'Payday (feat. Young Thug)' at track 3 with 2 s between them are one song.
        let p = plan(
            &[
                ("a", "Doja Cat", "Planet Her", 2021, "mb1", 1),
                ("b", "Doja Cat", "Planet Her", 2021, "", 2),
            ],
            &[
                ("l/3.mp3", "Payday (feat. Young Thug)", "a", 3, 211),
                ("s/3.mp3", "Payday", "b", 3, 213),
                ("s/4.mp3", "Get Into It (Yuh)", "b", 4, 138),
            ],
        );
        assert_eq!(p, [merge("l/3.mp3", &["s/4.mp3"], &["s/3.mp3"])]);
    }

    #[test]
    fn song_in_two_extra_copies_is_retagged_once() {
        // With 3 copies, a song that only the 2 smaller copies have joins the album once.
        let p = plan(
            &[
                ("a", "Metro Boomin", "Heroes", 2022, "mb1", 1),
                ("b", "Metro Boomin", "Heroes", 2022, "", 1),
                ("c", "Metro Boomin", "Heroes", 2022, "", 1),
            ],
            &[
                ("l/1.mp3", "Intro", "a", 1, 60),
                ("s/2.mp3", "On Time", "b", 2, 169),
                ("t/2.mp3", "On Time (with John Legend)", "c", 2, 170),
            ],
        );
        assert_eq!(p, [merge("l/1.mp3", &["s/2.mp3"], &["t/2.mp3"])]);
    }

    #[test]
    fn two_interludes_are_two_songs() {
        let p = plan(
            &[
                ("a", "Nas", "Album", 2006, "mb1", 1),
                ("b", "Nas", "Album", 2006, "", 1),
            ],
            &[
                ("l/4.mp3", "Interlude", "a", 4, 60),
                ("s/9.mp3", "Interlude", "b", 9, 95),
            ],
        );
        assert_eq!(p, [merge("l/4.mp3", &["s/9.mp3"], &[])]);
    }

    #[test]
    fn remix_is_not_a_duplicate_of_the_original() {
        let p = plan(
            &[
                ("a", "2Pac", "Loyal", 2004, "mb1", 1),
                ("b", "2Pac", "Loyal", 2004, "", 1),
            ],
            &[
                ("l/15.mp3", "Hennessey", "a", 15, 250),
                ("s/15.mp3", "Hennessey - Red Spyda Remix", "b", 15, 252),
            ],
        );
        assert_eq!(p, [merge("l/15.mp3", &["s/15.mp3"], &[])]);
    }

    #[test]
    fn reissue_from_another_year_joins_when_it_shares_a_song() {
        let p = plan(
            &[
                ("a", "Lady Gaga", "The Fame", 2008, "mb1", 2),
                ("b", "Lady Gaga", "The Fame", 2019, "", 2),
            ],
            &[
                ("l/1.mp3", "Just Dance", "a", 1, 242),
                ("l/2.mp3", "LoveGame", "a", 2, 216),
                ("s/1.mp3", "Just Dance (feat. Colby O'Donis)", "b", 1, 244),
                ("s/3.mp3", "Paparazzi", "b", 3, 208),
            ],
        );
        assert_eq!(p, [merge("l/1.mp3", &["s/3.mp3"], &["s/1.mp3"])]);
    }

    #[test]
    fn two_skits_of_similar_length_are_two_songs() {
        // A repeated title must match on track number; length alone is not enough.
        let p = plan(
            &[
                ("a", "X", "Tape", 2020, "mb1", 2),
                ("b", "X", "Tape", 2020, "", 2),
            ],
            &[
                ("l/5.mp3", "Skit", "a", 5, 45),
                ("l/6.mp3", "Song", "a", 6, 200),
                ("s/5.mp3", "Skit", "b", 5, 45),
                ("s/11.mp3", "Skit", "b", 11, 49),
            ],
        );
        assert_eq!(p, [merge("l/5.mp3", &["s/11.mp3"], &["s/5.mp3"])]);
    }

    #[test]
    fn same_name_in_other_year_is_a_different_album() {
        let p = plan(
            &[
                ("a", "Weezer", "Weezer", 1994, "", 10),
                ("b", "Weezer", "Weezer", 2001, "", 10),
            ],
            &[
                ("1/x.mp3", "Buddy Holly", "a", 1, 160),
                ("2/x.mp3", "Hash Pipe", "b", 1, 190),
            ],
        );
        assert_eq!(p, []);
    }

    #[test]
    fn two_musicbrainz_releases_are_left_alone() {
        // A single and its album, both from Lidarr, keep their own files.
        let p = plan(
            &[
                ("a", "Joey", "2000", 2022, "mb-album", 14),
                ("b", "Joey", "2000", 2022, "mb-single", 1),
            ],
            &[
                ("a/1.flac", "Where I Belong", "a", 1, 200),
                ("b/1.flac", "Where I Belong", "b", 1, 200),
            ],
        );
        assert_eq!(p, []);
    }

    #[test]
    fn untagged_copy_merges_while_a_single_with_its_own_id_stays() {
        let p = plan(
            &[
                ("a", "Joey", "2000", 2022, "mb-album", 2),
                ("b", "Joey", "2000", 2022, "mb-single", 1),
                ("c", "Joey", "2000", 2022, "", 1),
            ],
            &[
                ("a/1.flac", "Make Me Feel", "a", 2, 200),
                ("b/1.flac", "Make Me Feel", "b", 1, 200),
                ("c/9.mp3", "Survival Tactics", "c", 9, 220),
            ],
        );
        assert_eq!(p, [merge("a/1.flac", &["c/9.mp3"], &[])]);
    }

    #[test]
    fn main_moves_duplicates_retags_the_rest_and_logs_both() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            music: tmp.path().join("music"),
            duplicates: tmp.path().join("dups"),
            navidrome_db: tmp.path().join("navidrome.db"),
            undo_log: tmp.path().join("undo.jsonl"),
        };
        mp3(
            &paths.music.join("l/1.mp3"),
            vec![talb("Heroes"), txxx("MusicBrainz Album Id", "mb1")],
        );
        mp3(&paths.music.join("s/1.mp3"), vec![talb("HEROES")]);
        mp3(&paths.music.join("s/2.mp3"), vec![talb("HEROES")]);
        db(
            &[
                ("a", "Metro", "Heroes", 2022, "mb1", 1),
                ("b", "Metro", "Heroes", 2022, "", 2),
            ],
            &[
                ("l/1.mp3", "Intro", "a", 1, 60),
                ("s/1.mp3", "Intro", "b", 1, 61),
                ("s/2.mp3", "Outro", "b", 2, 90),
            ],
            Some(&paths.navidrome_db),
        );

        super::merge(&paths, true).unwrap();

        assert!(!paths.music.join("s/1.mp3").exists() && paths.duplicates.join("s/1.mp3").exists());
        let tags = album_tags(&paths.music.join("s/2.mp3")).unwrap();
        let want: Tags = vec![
            ("album".into(), vec!["Heroes".into()]),
            ("musicbrainz_albumid".into(), vec!["mb1".into()]),
        ];
        assert_eq!(tags, want);
        let log = std::fs::read_to_string(&paths.undo_log).unwrap();
        let moved = json_str(&paths.duplicates.join("s/1.mp3").display().to_string());
        let want = format!(
            "{{\"path\": \"s/1.mp3\", \"moved_to\": {moved}}}\n{{\"path\": \"s/2.mp3\", \"old\": {{\"album\": [\"HEROES\"]}}}}\n"
        );
        assert_eq!(log, want);
    }

    #[test]
    fn json_lines_match_python_json_dumps() {
        let old: Tags = vec![("album".into(), vec!["Beyoncé \"4\"".into(), "🎵\\".into()])];
        let line = retag_line("JAŸ-Z/x.mp3", &old);
        // ~ stands for a backslash, so the expected escapes stay readable here.
        let want = r#"{"path": "JA~u0178-Z/x.mp3", "old": {"album": ["Beyonc~u00e9 ~"4~"", "~ud83c~udfb5~~"]}}"#;
        assert_eq!(line, want.replace('~', "\\"));
        assert_eq!(parent("x.mp3"), ".");
        assert_eq!(parent("a/b/x.mp3"), "a/b");
    }

    #[test]
    #[ignore = "reads the live Navidrome database; run with --ignored"]
    fn live_plan_and_base_titles_match_the_python() {
        let paths = Paths::from_env();
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scripts/merge-split-albums.py"
        );
        let py = r#"
import importlib.util, json, sqlite3, sys
spec = importlib.util.spec_from_file_location("m", sys.argv[1])
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
db = sqlite3.connect(f"file:{sys.argv[2]}?mode=ro", uri=True)
titles = [t for (t,) in db.execute("select distinct title from media_file where title is not null")]
print(json.dumps({"plan": m.split_albums(db), "titles": {t: m.base_title(t) for t in titles}}))
"#;
        let out = std::process::Command::new("python3")
            .args(["-c", py, script, &paths.navidrome_db.display().to_string()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let want: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let db = Connection::open_with_flags(&paths.navidrome_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
        let plan = serde_json::to_value(split_albums(&db).unwrap()).unwrap();
        assert_eq!(plan, want["plan"]);
        let titles = want["titles"].as_object().unwrap();
        let bad: Vec<_> = titles
            .iter()
            .filter(|(t, b)| base_title(t) != b.as_str().unwrap())
            .collect();
        let files: usize = plan
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m[1].as_array().unwrap().len() + m[2].as_array().unwrap().len())
            .sum();
        println!(
            "{} split albums, {files} files to move or retag; {} titles, {} base titles differ",
            plan.as_array().unwrap().len(),
            titles.len(),
            bad.len()
        );
        assert!(bad.is_empty(), "{bad:?}");
    }
}

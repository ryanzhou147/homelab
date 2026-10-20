//! Tests of merge-albums' tags, which compare its results with the Python's through mutagen.

use super::*;
use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::id3::v2::{ExtendedTextFrame, Frame, FrameId, Id3v2Tag, TextInformationFrame};
use lofty::mpeg::MpegFile;
use lofty::tag::TagExt;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub(crate) fn talb(album: &str) -> Frame<'static> {
    let id = FrameId::new("TALB").unwrap();
    Frame::Text(TextInformationFrame::new(
        id,
        TextEncoding::UTF8,
        album.to_string(),
    ))
}

pub(crate) fn txxx(desc: &str, value: &str) -> Frame<'static> {
    Frame::UserText(ExtendedTextFrame::new(
        TextEncoding::UTF8,
        desc.to_string(),
        value.to_string(),
    ))
}

/// A tiny valid MP3: ID3 frames, then silent 128 kbps MPEG frames.
pub(crate) fn mp3(path: &Path, frames: Vec<Frame<'static>>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let frame: Vec<u8> = [0xff, 0xfb, 0x90, 0x64]
        .into_iter()
        .chain([0; 413])
        .collect();
    std::fs::write(path, frame.repeat(10)).unwrap();
    let mut tag = Id3v2Tag::new();
    for f in frames {
        tag.insert(f);
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), vec![v.to_string()]))
        .collect()
}

#[test]
fn retag_copies_upper_case_musicbrainz_id() {
    // A keeper with TXXX:MUSICBRAINZ_ALBUMID passes the ID on, and the old spelling goes away.
    let tmp = tempfile::tempdir().unwrap();
    let (keep, other) = (tmp.path().join("keep.mp3"), tmp.path().join("other.mp3"));
    mp3(
        &keep,
        vec![talb("Disorder"), txxx("MUSICBRAINZ_ALBUMID", "mb1")],
    );
    mp3(
        &other,
        vec![talb("DISORDER"), txxx("MUSICBRAINZ_ALBUMID", "mb2")],
    );
    let new = album_tags(&keep).unwrap();
    assert_eq!(
        new,
        tags(&[("album", "Disorder"), ("musicbrainz_albumid", "mb1")])
    );
    retag(&other, &new).unwrap();
    let mut file = File::open(&other).unwrap();
    let tag = MpegFile::read_from(&mut file, ParseOptions::new())
        .unwrap()
        .id3v2()
        .cloned()
        .unwrap();
    let ids: Vec<_> = tag
        .iter()
        .filter_map(|f| match f {
            Frame::UserText(t) => Some((t.description.to_string(), t.content.to_string())),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        [("MusicBrainz Album Id".to_string(), "mb1".to_string())]
    );
    assert_eq!(
        tag.get_text(&FrameId::new("TALB").unwrap()),
        Some("Disorder")
    );
}

// Runs the Python port's functions through mutagen. It needs python3 with mutagen, as the
// repo's Python tests do.
const PY: &str = r#"
import hashlib, importlib.util, json, sys
from pathlib import Path
from mutagen.id3 import ID3, APIC, COMM, TALB, TCON, TDOR, TDRC, TDRL, TIT2, TPE1, TPE2, TRCK, TSO2, TXXX
from mutagen.flac import FLAC, Picture

spec = importlib.util.spec_from_file_location("merge", sys.argv[1])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
cmd, args = sys.argv[2], json.load(sys.stdin)

def mp3(path, frames, v2=4, v1=b""):
    path.write_bytes((b"\xff\xfb\x90\x64" + bytes(413)) * 10)
    tags = ID3()
    for f in frames:
        tags.add(f)
    tags.save(path, v2_version=v2, v1=0)
    with open(path, "ab") as f:
        f.write(v1)

def v1(title, album, year, comment, track, genre):
    pad = lambda b, n: b[:n].ljust(n, b"\0")
    return (b"TAG" + pad(title, 30) + pad(b"V1 Artist", 30) + pad(album, 30) + pad(year, 4)
            + pad(comment, 28) + b"\0" + bytes([track, genre]))

def flac(path, tags, picture=False):
    info = (44100 << 44) | (1 << 41) | (15 << 36)
    # Some audio bytes after the metadata, as in a real file.
    path.write_bytes(b"fLaC" + bytes([0x80, 0, 0, 34]) + (4096).to_bytes(2, "big") * 2 + bytes(6)
                     + info.to_bytes(8, "big") + bytes(16) + b"\xff\xf8\x69\x08" + bytes(2000))
    f = FLAC(path)
    f.add_tags()
    for k, v in tags:
        f.tags.append((k, v))
    if picture:
        p = Picture()
        p.type, p.mime, p.data = 3, "image/png", b"\x89PNG fake"
        f.add_picture(p)
    f.save()

def fixtures(d):
    d = Path(d)
    mb = lambda desc, text: TXXX(encoding=3, desc=desc, text=text)
    mp3(d / "keep.mp3", [TIT2(encoding=3, text="Intro"), TALB(encoding=3, text="Heroes"),
        TPE2(encoding=3, text="Metro Boomin"), TSO2(encoding=3, text="Boomin, Metro"),
        TDRC(encoding=3, text="2022-12-02"), TDOR(encoding=3, text="2022"), TDRL(encoding=3, text="2022-12-02"),
        mb("MUSICBRAINZ_ALBUMID", "mb-1"), mb("MusicBrainz Album Artist Id", ["aa-1", "aa-2"]),
        mb("RELEASEDATE", "2022-12-02"), mb("MusicBrainz Release Group Id", "rg-é")])
    mp3(d / "other.mp3", [TIT2(encoding=1, text="On Time"), TPE1(encoding=3, text=["Metro", "Future"]),
        TALB(encoding=3, text="HEROES (Deluxe)"), TDRC(encoding=3, text="2022-12-05T10:00"),
        TDRL(encoding=3, text="2021"), mb("ORIGINALDATE", "2020"), mb("MusicBrainz Album Id", "mb-2"),
        mb("MUSICBRAINZ_RELEASEGROUPID", "rg-2"), mb("SPOTIFY", "keep me"), TCON(encoding=3, text="(17)Rock"),
        TRCK(encoding=3, text="2/14"), COMM(encoding=3, lang="eng", desc="", text="spotdl"),
        APIC(encoding=3, mime="image/jpeg", type=3, desc="", data=b"\xff\xd8 fake")],
        v1=v1(b"On Time", b"Old Album", b"2019", b"v1 note", 2, 7))
    mp3(d / "other23.mp3", [TIT2(encoding=1, text="Umbrella"), TALB(encoding=1, text="Heroes"),
        TDRC(encoding=1, text="2006-03-01"), TDOR(encoding=1, text="2005"), mb("MUSICBRAINZ_ALBUMID", "x"),
        TCON(encoding=1, text="Hip-Hop")], v2=3, v1=v1(b"Umbrella", b"Heroes", b"2006", b"", 300 % 256, 255))
    mp3(d / "v1only.mp3", [], v1=v1(b"Only V1", b"V1 Album", b"1999", b"", 0, 17))
    path = d / "v1only.mp3"
    data = path.read_bytes()
    path.write_bytes(data[data.index(b"\xff\xfb"):])
    flac(d / "keep.flac", [("ALBUM", "Heroes"), ("DATE", "2022"), ("MUSICBRAINZ_ALBUMID", "mb-f"),
        ("ALBUMARTIST", "Metro Boomin"), ("RELEASEDATE", "2022-12-02")])
    flac(d / "other.flac", [("TITLE", "Superhero"), ("album", "HEROES"), ("Album", "Second"), ("DATE", "2021"),
        ("TRACKNUMBER", "3/12"), ("musicbrainz_albumid", "old"), ("ORIGINALDATE", "2019"),
        ("ARTIST", "A"), ("ARTIST", "B")], picture=True)
    flac(d / "bare.flac", [])
    FLAC(d / "bare.flac").delete()

def short(frame):
    # Picture bytes as a hash, to keep the report short.
    if hasattr(frame, "mime"):
        return repr((frame.HashKey, frame.mime, frame.type, frame.desc, hashlib.sha1(frame.data).hexdigest()))
    return repr(frame)

def dump(path):
    path = Path(path)
    if path.suffix == ".flac":
        f = FLAC(path)
        return {"tags": list(f.tags) if f.tags is not None else None,
                "pictures": [repr((p.type, p.mime, p.desc, hashlib.sha1(p.data).hexdigest())) for p in f.pictures]}
    data = path.read_bytes()
    tags = ID3(path)
    return {"version": list(tags.version), "frames": sorted((k, short(v)) for k, v in tags.items()),
            "v1": data[-128:].hex() if data[-128:-125] == b"TAG" else None}

def retag(keep, target):
    # The Python stops on two TDRL bugs: mutagen gives TDRL values as ID3TimeStamp, which a FLAC
    # file and json.dumps refuse. Record the bug, and go on with the values as str.
    bug = None
    try:
        new = mod.album_tags(Path(keep))
        try:
            old = mod.retag(Path(target), new)
        except ValueError as e:
            bug = repr(e)
            old = mod.retag(Path(target), {k: [str(v) for v in vs] for k, vs in new.items()})
    except Exception as e:
        return {"error": repr(e)}
    try:
        json.dumps(old)
    except TypeError as e:
        bug = bug or repr(e)
    out = json.loads(json.dumps({"new": new, "old": old}, default=str))
    return out | {"python_bug": bug} if bug else out

if cmd == "fixtures":
    fixtures(args)
    out = None
elif cmd == "retag":
    out = [retag(k, t) for k, t in args]
else:
    out = []
    for p in args:
        try:
            out.append(dump(p))
        except Exception as e:
            out.append({"error": repr(e)})
json.dump(out, sys.stdout)
"#;

fn python(cmd: &str, args: &Value) -> Value {
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scripts/merge-split-albums.py"
    );
    let mut child = Command::new("python3")
        .args(["-c", PY, script, cmd])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3 runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "python3 {cmd} failed");
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The fields of two dumps that differ, with list items only one side has.
fn dump_diff(py: &Value, rs: &Value) -> String {
    let (Some(p), Some(r)) = (py.as_object(), rs.as_object()) else {
        return format!("py {py} rs {rs}");
    };
    let mut out = vec![];
    for (k, pv) in p {
        let rv = &r.get(k).cloned().unwrap_or_default();
        match (pv.as_array(), rv.as_array()) {
            _ if pv == rv => {}
            (Some(pa), Some(ra)) => {
                let only = |a: &Vec<Value>, b: &Vec<Value>| {
                    a.iter()
                        .filter(|x| !b.contains(x))
                        .cloned()
                        .collect::<Vec<_>>()
                };
                out.push(format!(
                    "{k}: py only {:?} rs only {:?}",
                    only(pa, ra),
                    only(ra, pa)
                ));
            }
            _ => out.push(format!("{k}: py {pv} rs {rv}")),
        }
    }
    out.join("; ")
}

fn tags_json(tags: &Tags) -> Value {
    Value::Object(tags.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// Retag each copy with Python and with Rust, then compare the results and mutagen's reading
/// of both files. Returns the pairs that differ, and the pairs where the Python stops on a bug.
fn compare(pairs: &[(PathBuf, PathBuf)], dir: &Path) -> (Vec<String>, Vec<String>) {
    let (mut py_pairs, mut rs_targets, mut rs_results) = (vec![], vec![], vec![]);
    for (i, (keep, target)) in pairs.iter().enumerate() {
        let ext = target.extension().unwrap().to_str().unwrap();
        let (py, rs) = (
            dir.join(format!("{i}.py.{ext}")),
            dir.join(format!("{i}.rs.{ext}")),
        );
        std::fs::copy(target, &py).unwrap();
        std::fs::copy(target, &rs).unwrap();
        let result = album_tags(keep).and_then(|new| Ok((retag(&rs, &new)?, new)));
        rs_results.push(match result {
            Ok((old, new)) => json!({"new": tags_json(&new), "old": tags_json(&old)}),
            Err(e) => json!({"error": format!("{e:#}")}),
        });
        py_pairs.push(json!([keep, py]));
        rs_targets.push(rs);
    }
    let mut py_results = python("retag", &json!(py_pairs));
    let py_paths: Vec<Value> = py_pairs.iter().map(|p| p[1].clone()).collect();
    let py_dumps = python("dump", &json!(py_paths));
    let rs_dumps = python("dump", &json!(rs_targets));
    let (mut bad, mut bugs) = (vec![], vec![]);
    for (i, (keep, target)) in pairs.iter().enumerate() {
        let name = format!("{} -> {}", keep.display(), target.display());
        if let Some(bug) = py_results[i].as_object_mut().unwrap().remove("python_bug") {
            bugs.push(format!("{name}: {bug}"));
        }
        let (pr, rr) = (&py_results[i], &rs_results[i]);
        // Both programs skip a file they cannot read; the messages differ.
        if pr.get("error").is_some() && rr.get("error").is_some() {
            continue;
        }
        if pr != rr || py_dumps[i] != rs_dumps[i] {
            let (pd, rd) = (&py_dumps[i], &rs_dumps[i]);
            bad.push(format!(
                "{name}\n  py {pr}\n  rs {rr}\n  dump {}",
                dump_diff(pd, rd)
            ));
        }
    }
    (bad, bugs)
}

#[test]
#[ignore = "needs python3, mutagen and scripts/merge-split-albums.py"]
fn retag_leaves_the_same_tags_as_the_python() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    python("fixtures", &json!(d));
    let names = [
        ("keep.mp3", "other.mp3"),
        ("keep.mp3", "other23.mp3"),
        ("keep.mp3", "v1only.mp3"),
        ("keep.flac", "other.mp3"),
        ("keep.mp3", "other.flac"),
        ("keep.flac", "other.flac"),
        ("keep.flac", "bare.flac"),
        ("other.mp3", "keep.mp3"),
        ("other23.mp3", "keep.flac"),
    ];
    let pairs: Vec<_> = names.iter().map(|(k, t)| (d.join(k), d.join(t))).collect();
    let work = d.join("work");
    std::fs::create_dir(&work).unwrap();
    let (bad, bugs) = compare(&pairs, &work);
    assert!(
        bad.is_empty(),
        "{} of {} differ:\n{}",
        bad.len(),
        pairs.len(),
        bad.join("\n")
    );
    // keep.mp3 and other.mp3 have TDRL: the Python cannot log it or copy it to a FLAC file.
    assert_eq!(bugs.len(), 4, "{bugs:?}");
}

fn music_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            music_files(&p, out);
        } else if matches!(p.extension().and_then(|e| e.to_str()), Some("mp3" | "flac")) {
            out.push(p);
        }
    }
}

/// A copy with the tags and some audio: the ID3v2 tag or FLAC metadata, 64 KiB, and the last
/// 4 KiB of an MP3 (ID3v1 and APE tags).
fn tag_copy(src: &Path, dst: &Path) {
    let data = std::fs::read(src).unwrap();
    let int = |b: &[u8]| b.iter().fold(0usize, |n, &x| n << 8 | x as usize);
    let (head, tail) = if data.starts_with(b"ID3") {
        let size = data[6..10].iter().fold(0usize, |n, &x| n << 7 | x as usize);
        (10 + size + if data[5] & 0x10 != 0 { 10 } else { 0 }, 4096)
    } else if data.starts_with(b"fLaC") {
        let mut pos = 4;
        while pos + 4 <= data.len() {
            let last = data[pos] & 0x80 != 0;
            pos += 4 + int(&data[pos + 1..pos + 4]);
            if last {
                break;
            }
        }
        (pos, 0)
    } else {
        (data.len(), 0)
    };
    let head = head + 65536;
    let copy = if head + tail >= data.len() {
        data
    } else {
        [&data[..head], &data[data.len() - tail..]].concat()
    };
    std::fs::write(dst, copy).unwrap();
}

#[test]
#[ignore = "reads the music library; run with --ignored"]
fn retag_of_library_copies_matches_the_python() {
    let media = std::env::var_os("MEDIA").map_or_else(
        || crate::common::home().join("homelab-media"),
        PathBuf::from,
    );
    let mut files = vec![];
    music_files(&media.join("music"), &mut files);
    let want: usize = std::env::var("MERGE_SAMPLE").map_or(400, |n| n.parse().unwrap());
    let step = (files.len() / want).max(1);
    let tmp = tempfile::tempdir().unwrap();
    let sample: Vec<PathBuf> = files
        .iter()
        .step_by(step)
        .enumerate()
        .map(|(i, src)| {
            let dst = tmp.path().join(format!(
                "lib{i}.{}",
                src.extension().unwrap().to_str().unwrap()
            ));
            tag_copy(src, &dst);
            dst
        })
        .collect();
    // Each file takes the album tags of the next one, so MP3 and FLAC keepers mix.
    let pairs: Vec<_> = (0..sample.len())
        .map(|i| (sample[(i + 1) % sample.len()].clone(), sample[i].clone()))
        .collect();
    let work = tmp.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let (bad, bugs) = compare(&pairs, &work);
    let flac = sample
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "flac"))
        .count();
    println!(
        "{} pairs ({flac} FLAC targets): {} differ, {} Python TDRL bugs",
        pairs.len(),
        bad.len(),
        bugs.len()
    );
    assert!(
        bad.is_empty(),
        "{} of {} differ:\n{}",
        bad.len(),
        pairs.len(),
        bad.join("\n")
    );
}

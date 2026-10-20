//! The ID3 tags of an MP3 file as mutagen loads and saves them, for merge-albums. The frames stay
//! raw bytes, so the frames that merge-albums does not change go back into the file as they were.
//! lofty's Id3v2Tag writes the tag, with each frame as a BinaryFrame.

use anyhow::{Context, Result, bail};
use lofty::config::WriteOptions;
use lofty::id3::v2::{BinaryFrame, Frame, FrameId, Id3v2Tag};
use lofty::tag::TagExt;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

// mutagen's ID3v1 genre list, split on |. It differs from lofty's in 24 spellings, and the v1
// genre byte must match.
const GENRES: &str = concat!(
    "Blues|Classic Rock|Country|Dance|Disco|Funk|Grunge|Hip-Hop|Jazz|Metal|New Age|Oldies|",
    "Other|Pop|R&B|Rap|Reggae|Rock|Techno|Industrial|Alternative|Ska|Death Metal|Pranks|",
    "Soundtrack|Euro-Techno|Ambient|Trip-Hop|Vocal|Jazz+Funk|Fusion|Trance|Classical|",
    "Instrumental|Acid|House|Game|Sound Clip|Gospel|Noise|Alt. Rock|Bass|Soul|Punk|Space|",
    "Meditative|Instrumental Pop|Instrumental Rock|Ethnic|Gothic|Darkwave|Techno-Industrial|",
    "Electronic|Pop-Folk|Eurodance|Dream|Southern Rock|Comedy|Cult|Gangsta Rap|Top 40|",
    "Christian Rap|Pop/Funk|Jungle|Native American|Cabaret|New Wave|Psychedelic|Rave|",
    "Showtunes|Trailer|Lo-Fi|Tribal|Acid Punk|Acid Jazz|Polka|Retro|Musical|Rock & Roll|",
    "Hard Rock|Folk|Folk-Rock|National Folk|Swing|Fast-Fusion|Bebop|Latin|Revival|Celtic|",
    "Bluegrass|Avantgarde|Gothic Rock|Progressive Rock|Psychedelic Rock|Symphonic Rock|",
    "Slow Rock|Big Band|Chorus|Easy Listening|Acoustic|Humour|Speech|Chanson|Opera|",
    "Chamber Music|Sonata|Symphony|Booty Bass|Primus|Porn Groove|Satire|Slow Jam|Club|Tango|",
    "Samba|Folklore|Ballad|Power Ballad|Rhythmic Soul|Freestyle|Duet|Punk Rock|Drum Solo|",
    "A Cappella|Euro-House|Dance Hall|Goa|Drum & Bass|Club-House|Hardcore|Terror|Indie|",
    "BritPop|Afro-Punk|Polsk Punk|Beat|Christian Gangsta Rap|Heavy Metal|Black Metal|",
    "Crossover|Contemporary Christian|Christian Rock|Merengue|Salsa|Thrash Metal|Anime|JPop|",
    "Synthpop|Abstract|Art Rock|Baroque|Bhangra|Big Beat|Breakbeat|Chillout|Downtempo|Dub|",
    "EBM|Eclectic|Electro|Electroclash|Emo|Experimental|Garage|Global|IDM|Illbient|",
    "Industro-Goth|Jam Band|Krautrock|Leftfield|Lounge|Math Rock|New Romantic|Nu-Breakz|",
    "Post-Punk|Post-Rock|Psytrance|Shoegaze|Space Rock|Trop Rock|World Music|Neoclassical|",
    "Audiobook|Audio Theatre|Neue Deutsche Welle|Podcast|Indie Rock|G-Funk|Dubstep|",
    "Garage Rock|Psybient",
);
const V1_COMMENT: &str = "ID3v1 Comment";
/// Frames that mutagen reads as time stamps.
const TIME_STAMPS: [&str; 5] = ["TDEN", "TDOR", "TDRC", "TDRL", "TDTG"];
/// mutagen's TextFrame classes. mutagen does not save one whose text is empty.
const TEXT_FRAMES: &str = concat!(
    "COMM GRP1 MVIN MVNM TALB TBPM TCAT TCMP TCOM TCON TCOP TDAT TDEN TDES TDLY TDOR TDRC TDRL ",
    "TDTG TENC TEXT TFLT TGID TIME TIT1 TIT2 TIT3 TKEY TKWD TLAN TLEN TMED TMOO TOAL TOFN TOLY ",
    "TOPE TORY TOWN TPE1 TPE2 TPE3 TPE4 TPOS TPRO TPUB TRCK TRDA TRSN TRSO TSIZ TSO2 TSOA TSOC ",
    "TSOP TSOT TSRC TSSE TSST TXXX TYER",
);

/// An ID3v2 tag as raw frames (ID, data), and whether the file ends in an ID3v1 tag.
pub struct Id3 {
    frames: Vec<(String, Vec<u8>)>,
    has_v1: bool,
}

fn synchsafe(b: &[u8]) -> usize {
    b.iter().fold(0, |n, &x| n << 7 | usize::from(x & 0x7f))
}

fn be_int(b: &[u8]) -> usize {
    b.iter().fold(0, |n, &x| n << 8 | usize::from(x))
}

/// Undo ID3 unsynchronisation: FF 00 becomes FF.
fn unsynch(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (i, &b) in data.iter().enumerate() {
        if !(b == 0 && i > 0 && data[i - 1] == 0xff) {
            out.push(b);
        }
    }
    out
}

/// mutagen's is_valid_frame_id: upper-case letters and digits, with at least one letter.
fn valid_id(id: &[u8]) -> bool {
    id.iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && id.iter().any(u8::is_ascii_uppercase)
}

/// mutagen's determine_bpi: true when the ID3v2.4 frame sizes are synchsafe. iTunes once wrote
/// plain integers.
fn synchsafe_sizes(data: &[u8]) -> bool {
    let walk = |size: fn(&[u8]) -> usize| -> (usize, i64) {
        let (mut o, mut found) = (0, 0);
        while o + 10 < data.len() {
            let part = &data[o..o + 10];
            if part.iter().all(|&b| b == 0) {
                return (found, -(((data.len() - o) % 10) as i64));
            }
            o += 10 + size(&part[4..8]);
            found += usize::from(valid_id(&part[..4]));
        }
        (found, o as i64 - data.len() as i64)
    };
    let ((as_bpi, bpi_off), (as_int, int_off)) = (walk(synchsafe), walk(be_int));
    !(as_int > as_bpi || (as_int == as_bpi && bpi_off >= 1 && int_off <= 1))
}

/// mutagen's ID3 frame reader for ID3v2.3 and ID3v2.4 tags at the start of the file.
fn read_v2(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    if data.len() < 10 || &data[..3] != b"ID3" {
        return Ok(vec![]);
    }
    let (major, flags) = (data[3], data[5]);
    if !matches!(major, 3 | 4) {
        // shortcut: ID3v2.2 files are skipped (none in the library), port mutagen's 2.2 upgrade
        // when one shows up.
        bail!("ID3v2.{major} tags are not supported");
    }
    let size = synchsafe(&data[6..10]);
    let mut body = data
        .get(10..10 + size)
        .context("the ID3v2 tag runs past the end of the file")?;
    if flags & 0x40 != 0 && body.len() >= 4 && !valid_id(&body[..4]) {
        // An extended header; some taggers set the flag without one, and frames follow at once.
        let ext = if major == 4 {
            synchsafe(&body[..4]).saturating_sub(4)
        } else {
            be_int(&body[..4])
        };
        body = body.get(4 + ext..).unwrap_or_default();
    }
    let tag_unsynch = flags & 0x80 != 0;
    let body = if tag_unsynch && major == 3 {
        unsynch(body)
    } else {
        body.to_vec()
    };
    let synchsafe_size = major == 4 && synchsafe_sizes(&body);
    let mut frames = vec![];
    let mut rest = &body[..];
    while rest.len() >= 10 && rest[..4].iter().any(|&b| b != 0) {
        let (head, tail) = rest.split_at(10);
        let size = if synchsafe_size {
            synchsafe(&head[4..8])
        } else {
            be_int(&head[4..8])
        };
        let (mut frame, after) = tail.split_at(size.min(tail.len()));
        rest = after;
        let fl = u16::from_be_bytes([head[8], head[9]]);
        if size == 0 || !valid_id(&head[..4]) {
            continue;
        }
        // shortcut: no zlib, so a file with a compressed frame is skipped (mutagen reads it), add
        // flate2 when the library has one.
        let compressed_or_encrypted = if major == 4 { 0x000c } else { 0x00c0 };
        if fl & compressed_or_encrypted != 0 {
            bail!("compressed or encrypted ID3 frames are not supported");
        }
        // ID3v2.4 flags: a data length indicator, then unsynchronisation per frame or tag.
        if major == 4 && fl & 0x0009 != 0 {
            frame = frame.get(4..).unwrap_or_default();
        }
        let frame = if major == 4 && (fl & 0x0002 != 0 || tag_unsynch) {
            unsynch(frame)
        } else {
            frame.to_vec()
        };
        frames.push((String::from_utf8_lossy(&head[..4]).into_owned(), frame));
    }
    Ok(frames)
}

/// One string of a text field, as mutagen's EncodedTextSpec reads it, and the bytes it used.
fn read_one(enc: u8, data: &[u8]) -> (String, usize) {
    let term = if matches!(enc, 1 | 2) { 2 } else { 1 };
    let end = if term == 1 {
        data.iter().position(|&b| b == 0)
    } else {
        (0..data.len().saturating_sub(1))
            .step_by(2)
            .find(|&i| data[i] == 0 && data[i + 1] == 0)
    };
    let (text, used) = match end {
        Some(e) => (&data[..e], e + term),
        None => (data, data.len()),
    };
    if text.len() < term {
        return (String::new(), used);
    }
    let utf16 = |be: bool, b: &[u8]| {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    let s = match enc {
        0 => text.iter().map(|&b| char::from(b)).collect(),
        1 => match text {
            [0xfe, 0xff, rest @ ..] => utf16(true, rest),
            [0xff, 0xfe, rest @ ..] => utf16(false, rest),
            _ => utf16(false, text),
        },
        2 => utf16(true, text),
        _ => String::from_utf8_lossy(text).into_owned(),
    };
    (s, used)
}

/// mutagen's MultiSpec of EncodedTextSpec: values until the data runs out, at least one.
fn read_many(enc: u8, mut data: &[u8]) -> Vec<String> {
    let mut values = vec![];
    loop {
        let (value, used) = read_one(enc, data);
        values.push(value);
        data = &data[used..];
        if data.is_empty() {
            return values;
        }
    }
}

fn encode(enc: u8, values: &[&str]) -> Vec<u8> {
    let mut out = vec![];
    for v in values {
        match enc {
            0 => out.extend(
                v.chars()
                    .map(|c| u8::try_from(c).unwrap_or(b'?'))
                    .chain([0]),
            ),
            1 => out.extend(
                [0xff, 0xfe]
                    .into_iter()
                    .chain(v.encode_utf16().flat_map(u16::to_le_bytes))
                    .chain([0, 0]),
            ),
            2 => out.extend(v.encode_utf16().flat_map(u16::to_be_bytes).chain([0, 0])),
            _ => out.extend(v.bytes().chain([0])),
        }
    }
    out
}

/// A text frame: the encoding byte, then the values.
fn text_frame(enc: u8, values: &[&str]) -> Vec<u8> {
    [enc].into_iter().chain(encode(enc, values)).collect()
}

/// The text values of a text frame data, or None for a bad encoding byte (mutagen drops it).
fn text_values(data: &[u8]) -> Option<(u8, Vec<String>)> {
    let (&enc, rest) = data.split_first()?;
    (enc <= 3).then(|| (enc, read_many(enc, rest)))
}

/// A TXXX frame's description and values.
fn txxx_values(data: &[u8]) -> Option<(String, Vec<String>)> {
    let (&enc, rest) = data.split_first()?;
    if enc > 3 {
        return None;
    }
    let (desc, used) = read_one(enc, rest);
    Some((desc, read_many(enc, &rest[used..])))
}

/// mutagen's ID3TimeStamp text: the numbers it can read, as 2006-03-01 10:00:00.
fn time_stamp(text: &str) -> String {
    static SPLIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[-T:/.]|\s+").expect("valid regex"));
    let padded = format!("{text}:::::");
    let mut out = String::new();
    for (i, part) in SPLIT.split(&padded).take(6).enumerate() {
        let Ok(n) = part.parse::<i64>() else { break };
        if i > 0 {
            out.push_str(["-", "-", " ", ":", ":"][i - 1]);
        }
        out.push_str(&if i == 0 {
            format!("{n:04}")
        } else {
            format!("{n:02}")
        });
    }
    out
}

/// True when mutagen reads the frame as a TextFrame with empty text, which its save leaves out.
fn empty_text(id: &str, data: &[u8]) -> bool {
    if !TEXT_FRAMES.split(' ').any(|t| t == id) {
        return false;
    }
    let values = match (id, data.split_first()) {
        (_, Some((&enc, _))) if enc > 3 => return false,
        ("TXXX", _) => txxx_values(data).map(|(_, v)| v),
        ("COMM", Some((&enc, rest))) if rest.len() >= 3 => {
            let (_, used) = read_one(enc, &rest[3..]);
            Some(read_many(enc, &rest[3 + used..]))
        }
        ("COMM", _) => return false,
        _ => text_values(data).map(|(_, v)| v),
    };
    // mutagen tests str(frame): the values joined with NUL, time stamps as it reads them.
    let stamp = TIME_STAMPS.contains(&id);
    let text = |t: &String| if stamp { time_stamp(t) } else { t.clone() };
    values.is_some_and(|v| v.iter().map(text).collect::<Vec<_>>().join("\0").is_empty())
}

impl Id3 {
    /// The tags of an MP3 file as mutagen loads them: the ID3v2 frames, the ID3v1 fields that
    /// they lack, then mutagen's update_to_v24.
    pub fn read(path: &Path) -> Result<Self> {
        let data = std::fs::read(path)?;
        let v23 = data.starts_with(b"ID3\x03");
        let mut id3 = Id3 {
            frames: read_v2(&data)?,
            has_v1: false,
        };
        if let Some((_, fields)) = find_v1(&data) {
            id3.has_v1 = true;
            id3.merge_v1(&fields, v23);
        }
        id3.update_to_v24();
        Ok(id3)
    }

    fn has(&self, id: &str) -> bool {
        self.frames.iter().any(|(i, _)| i == id)
    }

    /// The values of the first readable text frame with this ID. Time stamps are normalized
    /// like mutagen's ID3TimeStamp.
    pub fn text(&self, id: &str) -> Option<Vec<String>> {
        let (_, values) = self
            .frames
            .iter()
            .filter(|(i, _)| i == id)
            .find_map(|(_, d)| text_values(d))?;
        Some(if TIME_STAMPS.contains(&id) {
            values.iter().map(|v| time_stamp(v)).collect()
        } else {
            values
        })
    }

    pub fn txxx(&self, desc: &str) -> Option<Vec<String>> {
        let mut found = self
            .frames
            .iter()
            .filter(|(i, _)| i == "TXXX")
            .filter_map(|(_, d)| txxx_values(d));
        found.find(|(d, _)| d == desc).map(|(_, values)| values)
    }

    pub fn remove(&mut self, id: &str) {
        self.frames.retain(|(i, _)| i != id);
    }

    pub fn remove_txxx(&mut self, desc: &str) {
        self.frames
            .retain(|(i, d)| i != "TXXX" || txxx_values(d).is_none_or(|(x, _)| x != desc));
    }

    /// Add a text frame in UTF-8, as EasyID3 writes it. Time stamps get a T before the time.
    pub fn add_text(&mut self, id: &str, values: &[String]) {
        let stamp = TIME_STAMPS.contains(&id);
        let values: Vec<String> = values
            .iter()
            .map(|v| {
                if stamp {
                    v.replace(' ', "T")
                } else {
                    v.clone()
                }
            })
            .collect();
        let values: Vec<&str> = values.iter().map(String::as_str).collect();
        self.frames.push((id.into(), text_frame(3, &values)));
    }

    /// Add a TXXX frame. EasyID3 stores it in Latin-1 when every value is ASCII.
    pub fn add_txxx(&mut self, desc: &str, values: &[String]) {
        let enc = if values.iter().all(|v| v.is_ascii()) {
            0
        } else {
            3
        };
        let values: Vec<&str> = values.iter().map(String::as_str).collect();
        let data = [enc]
            .into_iter()
            .chain(encode(enc, &[desc]))
            .chain(encode(enc, &values))
            .collect();
        self.frames.push(("TXXX".into(), data));
    }

    /// mutagen adds each ID3v1 field that the ID3v2 tag lacks. An ID3v2.3 tag gets the year
    /// as TYER, which update_to_v24 turns into TDRC.
    fn merge_v1(&mut self, fields: &V1Fields, v23: bool) {
        for (id, value) in fields {
            if *id == "COMM" {
                let is_v1 = |d: &Vec<u8>| -> bool {
                    let Some((&enc, rest)) = d.split_first() else {
                        return false;
                    };
                    enc <= 3
                        && rest.len() >= 3
                        && &rest[..3] == b"eng"
                        && read_one(enc, &rest[3..]).0 == V1_COMMENT
                };
                if !self.frames.iter().any(|(i, d)| i == "COMM" && is_v1(d)) {
                    let data = [0]
                        .into_iter()
                        .chain(*b"eng")
                        .chain(encode(0, &[V1_COMMENT, value]))
                        .collect();
                    self.frames.push(("COMM".into(), data));
                }
                continue;
            }
            let id = if *id == "TDRC" && v23 { "TYER" } else { id };
            if !self.has(id) {
                self.frames.push((id.into(), text_frame(0, &[value])));
            }
        }
    }

    /// The text values of the first frame with this ID, and remove every frame with it.
    fn pop(&mut self, id: &str) -> Vec<String> {
        let values = self
            .frames
            .iter()
            .filter(|(i, _)| i == id)
            .find_map(|(_, d)| text_values(d));
        self.remove(id);
        values.map(|(_, v)| v).unwrap_or_default()
    }

    /// mutagen's update_to_v24, which runs on every load: TCON becomes genre names, TYER, TDAT
    /// and TIME become TDRC, TORY becomes TDOR, IPLS becomes TIPL, and frames ID3v2.4 lacks go.
    fn update_to_v24(&mut self) {
        // shortcut: no fix of APIC mime types "PNG" and "JPG"; they come from ID3v2.2 tags.
        if let Some(pos) = self
            .frames
            .iter()
            .position(|(i, d)| i == "TCON" && text_values(d).is_some())
        {
            let (enc, values) = text_values(&self.frames[pos].1).unwrap_or_default();
            let names = genres(&values);
            self.frames[pos].1 =
                text_frame(enc, &names.iter().map(String::as_str).collect::<Vec<_>>());
        }
        static YEAR: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^([0-9]{4})(-[0-9]{2}-[0-9]{2})?$").expect("valid"));
        static PAIR: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^([0-9]{2})([0-9]{2})$").expect("valid"));
        let (years, dates, times) = (self.pop("TYER"), self.pop("TDAT"), self.pop("TIME"));
        let mut stamps = vec![];
        for i in 0..years.len().max(dates.len()).max(times.len()) {
            let at = |v: &[String]| v.get(i).cloned().unwrap_or_default();
            let year = at(&years);
            let Some(ym) = YEAR.captures(&year) else {
                continue;
            };
            let mut stamp = ym[1].to_string();
            let mut month_day = ym.get(2).map(|m| m.as_str().to_string());
            if let Some(dm) = PAIR.captures(&at(&dates)) {
                month_day = Some(format!("-{}-{}", &dm[2], &dm[1]));
            }
            if let Some(md) = month_day {
                stamp.push_str(&md);
                if let Some(tm) = PAIR.captures(&at(&times)) {
                    stamp.push_str(&format!("T{}:{}:00", &tm[1], &tm[2]));
                }
            }
            stamps.push(stamp);
        }
        if !stamps.is_empty() && !self.has("TDRC") {
            self.frames.push((
                "TDRC".into(),
                text_frame(0, &stamps.iter().map(String::as_str).collect::<Vec<_>>()),
            ));
        }
        if self.has("TORY") {
            let original = self.pop("TORY").join("\0");
            if !self.has("TDOR") {
                self.frames
                    .push(("TDOR".into(), text_frame(0, &[&original])));
            }
        }
        if let Some(pos) = self.frames.iter().position(|(i, _)| i == "IPLS") {
            let (_, data) = self.frames.remove(pos);
            self.remove("IPLS");
            if !self.has("TIPL") {
                self.frames.push(("TIPL".into(), data));
            }
        }
        for id in ["RVAD", "EQUA", "TRDA", "TSIZ"] {
            self.remove(id);
        }
    }

    /// Write the frames as an ID3v2.4 tag, and update the ID3v1 tag if the file has one, as
    /// mutagen's save does. lofty makes the tag bytes; its save_to_path cannot find the format of
    /// some MP3 files that mutagen reads.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut tag = Id3v2Tag::new();
        for (id, data) in self.frames.iter().filter(|(id, d)| !empty_text(id, d)) {
            let id = FrameId::new(id.clone())?;
            tag.insert(Frame::Binary(BinaryFrame::new(id, data.clone())));
        }
        let mut out = vec![];
        tag.dump_to(&mut out, WriteOptions::default())?;
        let data = std::fs::read(path)?;
        let old_size = match data.get(..10) {
            Some(h) if h.starts_with(b"ID3") => {
                10 + synchsafe(&h[6..10]) + if h[5] & 0x10 != 0 { 10 } else { 0 }
            }
            _ => 0,
        };
        let mut rest = data
            .get(old_size..)
            .context("the ID3v2 tag runs past the end of the file")?;
        if self.has_v1 {
            let (size, _) = find_v1(rest).context("the ID3v1 tag went away")?;
            rest = &rest[..rest.len() - size];
        }
        out.extend_from_slice(rest);
        if self.has_v1 {
            out.extend(self.make_v1());
        }
        crate::common::write_atomic(path, &out)?;
        Ok(())
    }

    /// mutagen's MakeID3v1: an ID3v1.1 tag from the ID3v2 frames.
    fn make_v1(&self) -> Vec<u8> {
        let first = |id: &str| self.text(id).and_then(|v| v.into_iter().next());
        let mut out = b"TAG".to_vec();
        for id in ["TIT2", "TPE1", "TALB"] {
            let mut b: Vec<u8> = first(id)
                .unwrap_or_default()
                .chars()
                .map(|c| u8::try_from(c).unwrap_or(b'?'))
                .collect();
            b.resize(30, 0);
            out.extend(b);
        }
        let year = self.text("TDRC").map(|v| v.join("\0")).unwrap_or_default();
        let mut year: Vec<u8> = year
            .chars()
            .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
            .collect();
        year.resize(4, 0);
        out.extend(year);
        let track = first("TRCK")
            .and_then(|t| {
                t.split('/')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .parse::<i64>()
                    .ok()
            })
            .and_then(|n| u8::try_from(n).ok());
        // mutagen looks for a frame keyed "COMM", which never exists, so the comment stays empty.
        match track {
            Some(n) => out.extend([0u8; 28].into_iter().chain([0, n])),
            None => out.extend([0u8; 30]),
        }
        let genre = self
            .text("TCON")
            .and_then(|v| genres(&v).into_iter().next());
        let index = genre.and_then(|g| GENRES.split('|').position(|x| x == g));
        out.push(index.and_then(|i| u8::try_from(i).ok()).unwrap_or(255));
        out
    }
}

/// The ID3v1 fields as frames, like mutagen's ParseID3v1: (frame ID, text).
type V1Fields = Vec<(&'static str, String)>;

/// mutagen's find_id3v1: the size from the tag start to the file end, and the fields.
fn find_v1(data: &[u8]) -> Option<(usize, V1Fields)> {
    // Read 3 more bytes so an APEv2 footer ("APETAGEX") is not taken for an ID3v1 "TAG".
    let tail = &data[data.len().saturating_sub(131)..];
    let idx = tail.windows(3).position(|w| w == b"TAG")?;
    let ape = tail.windows(8).position(|w| w == b"APETAGEX");
    if ape.is_some_and(|a| idx == a + 5) {
        return None;
    }
    let tag = &tail[idx..];
    (124..=128)
        .contains(&tag.len())
        .then(|| (tag.len(), parse_v1(tag)))
}

fn parse_v1(tag: &[u8]) -> V1Fields {
    let year_len = tag.len() - 124;
    // Text up to the first NUL, without outer white space, read as Latin-1.
    let fix = |b: &[u8]| -> String {
        let b = b.split(|&c| c == 0).next().unwrap_or_default();
        let s: String = b.iter().map(|&c| char::from(c)).collect();
        s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'))
            .to_string()
    };
    let (title, artist, album) = (&tag[3..33], &tag[33..63], &tag[63..93]);
    let year = &tag[93..93 + year_len];
    let mut comment = &tag[93 + year_len..123 + year_len];
    let genre = tag[123 + year_len];
    let mut track = 0;
    if comment[28] == 0 {
        track = comment[29];
        comment = &comment[..28];
    }
    let mut fields = vec![];
    for (id, value) in [
        ("TIT2", title),
        ("TPE1", artist),
        ("TALB", album),
        ("TDRC", year),
        ("COMM", comment),
    ] {
        fields.push((id, fix(value)));
    }
    if track != 0 {
        fields.push(("TRCK", track.to_string()));
    }
    if genre != 255 {
        fields.push(("TCON", genre.to_string()));
    }
    fields.retain(|(_, v)| !v.is_empty());
    fields
}

fn genre(n: usize) -> String {
    GENRES.split('|').nth(n).unwrap_or("Unknown").to_string()
}

/// mutagen's TCON.genres: ID3v1 numbers and "(17)Rock" forms become genre names.
fn genres(values: &[String]) -> Vec<String> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^((?:\(([0-9]+|RX|CR)\))*)(.+)?").expect("valid regex"));
    let mut out = vec![];
    for value in values {
        let number = value
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| value.parse::<usize>().ok());
        match (value.as_str(), number.flatten()) {
            ("", _) => {}
            (_, Some(n)) if n < 256 => out.push(genre(n)),
            ("CR", _) => out.push("Cover".into()),
            ("RX", _) => out.push("Remix".into()),
            _ => {
                let Some(caps) = RE.captures(value) else {
                    continue;
                };
                let ids = caps.get(1).map_or("", |m| m.as_str());
                let mut found: Vec<String> = vec![];
                if !ids.is_empty() {
                    for gid in ids[1..ids.len() - 1].split(")(") {
                        found.push(match gid {
                            "CR" => "Cover".into(),
                            "RX" => "Remix".into(),
                            g => genre(g.parse::<usize>().unwrap_or(usize::MAX)),
                        });
                    }
                }
                if let Some(name) = caps.get(3).map(|m| m.as_str()) {
                    let name = if name.starts_with("((") {
                        &name[1..]
                    } else {
                        name
                    };
                    if !found.iter().any(|g| g == name) {
                        found.push(name.into());
                    }
                }
                out.extend(found);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genres_follow_mutagen() {
        let g = |v: &[&str]| genres(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).join("|");
        assert_eq!(
            g(&["17", "(1)(9)Rock", "(17)Rock"]),
            "Rock|Classic Rock|Metal|Rock|Rock"
        );
        assert_eq!(
            g(&["((foo)", "300", "", "RX", "(255)"]),
            "(foo)|300|Remix|Unknown"
        );
        assert_eq!(GENRES.split('|').count(), 192);
    }

    #[test]
    fn v1_fields_and_time_stamps_follow_mutagen() {
        let mut tag = b"TAG".to_vec();
        for field in [&b"Title"[..], b"Artist", b"Album "] {
            tag.extend(field.iter().copied().chain(std::iter::repeat(0)).take(30));
        }
        tag.extend(b"1999");
        tag.extend(
            b"note"
                .iter()
                .copied()
                .chain(std::iter::repeat(0))
                .take(28)
                .chain([0, 7, 17]),
        );
        let want = [
            ("TIT2", "Title"),
            ("TPE1", "Artist"),
            ("TALB", "Album"),
            ("TDRC", "1999"),
        ];
        let want = want
            .into_iter()
            .chain([("COMM", "note"), ("TRCK", "7"), ("TCON", "17")]);
        assert_eq!(
            parse_v1(&tag),
            want.map(|(k, v)| (k, v.to_string())).collect::<Vec<_>>()
        );
        assert_eq!(time_stamp("2006-3-1T10:00"), "2006-03-01 10:00");
        assert_eq!(time_stamp("2006/03/01"), "2006-03-01");
        assert_eq!(time_stamp("abc"), "");
    }

    #[test]
    fn text_frames_read_like_mutagen() {
        assert_eq!(
            text_values(b"\x03a\0b\0"),
            Some((3, vec!["a".into(), "b".into()]))
        );
        assert_eq!(text_values(b"\x03"), Some((3, vec![String::new()])));
        assert_eq!(
            text_values(b"\x01\xff\xfeh\0i\0\0\0"),
            Some((1, vec!["hi".into()]))
        );
        assert_eq!(text_values(b"\x07x"), None);
        let data = text_frame(1, &["é", "x"]);
        assert_eq!(text_values(&data), Some((1, vec!["é".into(), "x".into()])));
    }
}

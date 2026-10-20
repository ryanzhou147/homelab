//! Album tags of MP3 and FLAC files for merge-albums. Reads and writes them as mutagen's EasyID3
//! and FLAC classes do in scripts/merge-split-albums.py, so both programs leave the same tags.

use crate::merge_id3::Id3;
use anyhow::{Result, bail};
use lofty::file::FileType;
use lofty::probe::Probe;
use std::path::Path;

/// Tag names and their values, in ALBUM_TAGS order, like the Python dicts.
pub type Tags = Vec<(String, Vec<String>)>;

/// The tags Navidrome reads to tell albums apart (its PIDAlbum setting), in mutagen's easy names.
pub const ALBUM_TAGS: [&str; 9] = [
    "album",
    "albumartist",
    "albumartistsort",
    "date",
    "originaldate",
    "releasedate",
    "musicbrainz_albumid",
    "musicbrainz_albumartistid",
    "musicbrainz_releasegroupid",
];
// Some taggers write TXXX:MUSICBRAINZ_ALBUMID where mutagen expects TXXX:MusicBrainz Album Id.
// Navidrome reads both, so read both and write only mutagen's.
const UPPER_TXXX: [&str; 5] = [
    "musicbrainz_albumid",
    "musicbrainz_albumartistid",
    "musicbrainz_releasegroupid",
    "originaldate",
    "releasedate",
];

/// The mutagen names to read for one ALBUM_TAGS key: the key, then its upper-case TXXX twin.
fn names(key: &str) -> Vec<String> {
    let mut out = vec![key.to_string()];
    if UPPER_TXXX.contains(&key) {
        out.push(format!("{key}_upper"));
    }
    out
}

/// Where an EasyID3 name lives: a text frame, or a TXXX frame with this description.
enum Slot {
    Text(&'static str),
    Txxx(String),
}

fn id3_slot(name: &str) -> Result<Slot> {
    Ok(match name {
        "album" => Slot::Text("TALB"),
        "albumartist" => Slot::Text("TPE2"),
        "albumartistsort" => Slot::Text("TSO2"),
        "date" => Slot::Text("TDRC"),
        "originaldate" => Slot::Text("TDOR"),
        // mutagen's easy names lack TDRL; the Python registers it as releasedate.
        "releasedate" => Slot::Text("TDRL"),
        "musicbrainz_albumid" => Slot::Txxx("MusicBrainz Album Id".into()),
        "musicbrainz_albumartistid" => Slot::Txxx("MusicBrainz Album Artist Id".into()),
        "musicbrainz_releasegroupid" => Slot::Txxx("MusicBrainz Release Group Id".into()),
        n => match n.strip_suffix("_upper") {
            Some(key) => Slot::Txxx(key.to_uppercase()),
            None => bail!("no ID3 frame for {n}"),
        },
    })
}

/// EasyID3 reads albumartistsort from TSO2, else from TXXX:ALBUMARTISTSORT, and deletes both.
const SORT_TXXX: &str = "ALBUMARTISTSORT";

/// The tags of an open MP3 or FLAC file, with mutagen's easy names.
enum Media {
    Mp3(Id3),
    /// The Vorbis comments as mutagen reads them (None without a comment block), and the vendor.
    Flac(Option<Vec<(String, String)>>, String),
}

impl Media {
    fn open(path: &Path) -> Result<Self> {
        match Probe::open(path)?.guess_file_type()?.file_type() {
            Some(FileType::Mpeg) => Ok(Media::Mp3(Id3::read(path)?)),
            Some(FileType::Flac) => {
                let data = std::fs::read(path)?;
                let blocks = flac_blocks(&data)?.0;
                let mut comments = blocks.iter().filter(|(ty, _)| *ty == VORBIS_COMMENT);
                let found = comments
                    .next()
                    .map(|(_, b)| parse_vorbis_comments(b))
                    .transpose()?;
                if comments.next().is_some() {
                    bail!("more than one Vorbis comment block");
                }
                Ok(match found {
                    Some((items, vendor)) => Media::Flac(Some(items), vendor),
                    None => Media::Flac(None, String::new()),
                })
            }
            other => bail!("{}: unsupported file type {other:?}", path.display()),
        }
    }

    /// The values of one name, or None when the file lacks it.
    fn get(&self, name: &str) -> Result<Option<Vec<String>>> {
        Ok(match self {
            Media::Mp3(id3) => match id3_slot(name)? {
                Slot::Text("TSO2") => id3.text("TSO2").or_else(|| id3.txxx(SORT_TXXX)),
                Slot::Text(id) => id3.text(id),
                Slot::Txxx(desc) => id3.txxx(&desc),
            },
            Media::Flac(items, _) => {
                let lower = name.to_lowercase();
                let values: Vec<String> = items
                    .iter()
                    .flatten()
                    .filter(|(k, _)| k.to_lowercase() == lower)
                    .map(|(_, v)| v.clone())
                    .collect();
                (!values.is_empty()).then_some(values)
            }
        })
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        match self {
            Media::Mp3(id3) => match id3_slot(name)? {
                Slot::Text(id) => {
                    id3.remove(id);
                    if id == "TSO2" {
                        id3.remove_txxx(SORT_TXXX);
                    }
                }
                Slot::Txxx(desc) => id3.remove_txxx(&desc),
            },
            Media::Flac(items, _) => {
                let lower = name.to_lowercase();
                if let Some(items) = items {
                    items.retain(|(k, _)| k.to_lowercase() != lower);
                }
            }
        }
        Ok(())
    }

    /// Add the name with these values, after delete() removed the old ones.
    fn set(&mut self, name: &str, values: &[String]) -> Result<()> {
        match self {
            Media::Mp3(id3) => match id3_slot(name)? {
                Slot::Text(id) => id3.add_text(id, values),
                Slot::Txxx(desc) => id3.add_txxx(&desc, values),
            },
            // mutagen adds a comment block when the file has none, and keeps the key as given.
            Media::Flac(items, _) => {
                let items = items.get_or_insert_default();
                items.extend(values.iter().map(|v| (name.to_string(), v.clone())));
            }
        }
        Ok(())
    }

    fn save(self, path: &Path) -> Result<()> {
        match self {
            Media::Mp3(id3) => id3.save(path),
            Media::Flac(items, vendor) => {
                let data = std::fs::read(path)?;
                let (mut blocks, start, end) = flac_blocks(&data)?;
                let block;
                if let Some(items) = items {
                    block = vorbis_comment_block(&items, &vendor)?;
                    match blocks.iter().position(|(ty, _)| *ty == VORBIS_COMMENT) {
                        Some(p) => blocks[p].1 = &block,
                        None => blocks.push((VORBIS_COMMENT, &block)),
                    }
                }
                let mut out = data[..start].to_vec();
                out.extend(b"fLaC");
                for (i, (ty, block)) in blocks.iter().enumerate() {
                    let last = if i + 1 == blocks.len() { 0x80 } else { 0 };
                    let size = u32::try_from(block.len()).ok().filter(|&n| n < 1 << 24);
                    let Some(size) = size else {
                        bail!("a FLAC metadata block is too large")
                    };
                    out.push(ty | last);
                    out.extend(&size.to_be_bytes()[1..]);
                    out.extend(*block);
                }
                out.extend(&data[end..]);
                crate::common::write_atomic(path, &out)?;
                Ok(())
            }
        }
    }
}

const VORBIS_COMMENT: u8 = 4;
/// FLAC metadata blocks: (type, data).
type Blocks<'a> = Vec<(u8, &'a [u8])>;

/// The FLAC metadata blocks (type, data), where "fLaC" starts, and where the audio starts.
/// lofty upper-cases TRACKNUMBER on read and drops empty values on write, so merge-albums reads
/// and writes the Vorbis comment block itself, as mutagen does.
fn flac_blocks(data: &[u8]) -> Result<(Blocks<'_>, usize, usize)> {
    let mut pos = 0;
    if data.starts_with(b"ID3") && data.len() >= 10 {
        pos = 10
            + data[6..10]
                .iter()
                .fold(0, |n, &x| n << 7 | usize::from(x & 0x7f));
    }
    if data.get(pos..pos + 4) != Some(b"fLaC") {
        bail!("not a FLAC file");
    }
    let start = pos;
    pos += 4;
    let mut blocks = vec![];
    loop {
        let Some(head) = data.get(pos..pos + 4) else {
            bail!("the FLAC metadata runs past the end of the file");
        };
        let size = head[1..].iter().fold(0, |n, &x| n << 8 | usize::from(x));
        let Some(block) = data.get(pos + 4..pos + 4 + size) else {
            bail!("a FLAC metadata block runs past the end of the file");
        };
        blocks.push((head[0] & 0x7f, block));
        pos += 4 + size;
        if head[0] & 0x80 != 0 {
            return Ok((blocks, start, pos));
        }
    }
}

fn vorbis_comment_block(items: &[(String, String)], vendor: &str) -> Result<Vec<u8>> {
    let len = |s: &str| u32::try_from(s.len()).map(u32::to_le_bytes);
    let mut out = len(vendor)?.to_vec();
    out.extend(vendor.as_bytes());
    out.extend(u32::try_from(items.len())?.to_le_bytes());
    for (k, v) in items {
        let item = format!("{k}={v}");
        out.extend(len(&item)?);
        out.extend(item.as_bytes());
    }
    Ok(out)
}

/// mutagen's reading of a Vorbis comment block: (key, value) items and the vendor.
fn parse_vorbis_comments(block: &[u8]) -> Result<(Vec<(String, String)>, String)> {
    let mut at = 0;
    let mut take = |n: usize| -> Result<&[u8]> {
        let Some(b) = block.get(at..at + n) else {
            bail!("short Vorbis comment block")
        };
        at += n;
        Ok(b)
    };
    let u32le = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let vendor_len = u32le(take(4)?);
    let vendor = String::from_utf8_lossy(take(vendor_len)?).into_owned();
    let count = u32le(take(4)?);
    let mut items = vec![];
    for i in 0..count {
        let len = u32le(take(4)?);
        let text = String::from_utf8_lossy(take(len)?).into_owned();
        // mutagen names a field without "=" unknownN, puts ? for other characters in a key,
        // and drops a key that is empty or has characters outside 0x20 to 0x7D.
        let (key, value) = match text.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (format!("unknown{i}"), text.clone()),
        };
        let key: String = key
            .chars()
            .map(|c| if c.is_ascii() { c } else { '?' })
            .collect();
        if !key.is_empty() && key.bytes().all(|b| (0x20..=0x7d).contains(&b)) {
            items.push((key, value));
        }
    }
    Ok((items, vendor))
}

/// The album tags of a file, with each key's first spelling that the file has.
pub fn album_tags(path: &Path) -> Result<Tags> {
    let media = Media::open(path)?;
    let mut found = vec![];
    for key in ALBUM_TAGS {
        for name in names(key) {
            if let Some(values) = media.get(&name)? {
                if !values.is_empty() {
                    found.push((key.to_string(), values));
                }
                break;
            }
        }
    }
    Ok(found)
}

/// Give the file exactly the album tags in new, and return its old album tags.
pub fn retag(path: &Path, new: &Tags) -> Result<Tags> {
    let mut media = Media::open(path)?;
    let mut old = vec![];
    for key in ALBUM_TAGS {
        for name in names(key) {
            if let Some(values) = media.get(&name)? {
                old.push((name, values));
            }
        }
    }
    for key in ALBUM_TAGS {
        for name in names(key) {
            media.delete(&name)?;
        }
        if let Some((_, values)) = new.iter().find(|(k, _)| k == key) {
            media.set(key, values)?;
        }
    }
    media.save(path)?;
    Ok(old)
}

#[cfg(test)]
#[path = "merge_parity.rs"]
pub(crate) mod tests;

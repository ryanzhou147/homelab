//! Tests of merge-albums' tags.

use super::*;
use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::id3::v2::{ExtendedTextFrame, Frame, FrameId, Id3v2Tag, TextInformationFrame};
use lofty::mpeg::MpegFile;
use lofty::tag::TagExt;
use std::fs::File;

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

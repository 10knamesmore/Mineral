//! Reads embedded lyric text and decodes sidecar files for the shared LRC parser.

use std::path::Path;

use color_eyre::eyre::bail;
use lofty::{
    file::{TaggedFile, TaggedFileExt},
    tag::{ItemKey, Tag},
};
use mineral_model::{Lyrics, has_timed, has_words, parse_lrc};

/// Prefers word timing, then line timing, then text; keeps equal-quality candidates in primary-tag order.
pub(super) fn read(tagged: &TaggedFile, path: &Path) -> Option<Lyrics> {
    let mut selected = None;
    let primary = tagged.primary_tag();
    for tag in primary.into_iter().chain(
        tagged
            .tags()
            .iter()
            .filter(|tag| Some(tag.tag_type()) != primary.map(Tag::tag_type)),
    ) {
        for text in tag.get_strings(&ItemKey::Lyrics) {
            select(
                &mut selected,
                Lyrics {
                    lines: parse_lrc(text),
                },
            );
        }
    }
    if let Some(lyrics) = &selected {
        mineral_log::debug!(target: "local_library", path = %path.display(), lines = lyrics.lines.len(), timed = has_timed(&lyrics.lines), words = has_words(&lyrics.lines), "embedded lyrics selected");
    }
    selected
}

/// Skips blank candidates and replaces the selection only when more timing information is available.
fn select(selected: &mut Option<Lyrics>, lyrics: Lyrics) {
    if !lyrics
        .lines
        .iter()
        .any(|line| !line.kind.text().trim().is_empty())
    {
        return;
    }
    let quality = |lyrics: &Lyrics| (has_words(&lyrics.lines), has_timed(&lyrics.lines));
    if selected
        .as_ref()
        .is_none_or(|current| quality(&lyrics) > quality(current))
    {
        *selected = Some(lyrics);
    }
}

/// Decodes UTF-8 or BOM-declared UTF-16 without guessing undeclared legacy encodings.
pub(crate) fn decode_sidecar(bytes: &[u8]) -> color_eyre::Result<String> {
    match bytes {
        [0xff, 0xfe, rest @ ..] => decode_utf16(rest, true),
        [0xfe, 0xff, rest @ ..] => decode_utf16(rest, false),
        [0xef, 0xbb, 0xbf, rest @ ..] => Ok(std::str::from_utf8(rest)?.to_owned()),
        _ => Ok(std::str::from_utf8(bytes)?.to_owned()),
    }
}

/// Rejects truncated code units and invalid surrogate pairs instead of changing lyric text.
fn decode_utf16(bytes: &[u8], little_endian: bool) -> color_eyre::Result<String> {
    let (pairs, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        bail!("odd UTF-16 lyric byte length");
    }
    let units = pairs
        .iter()
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes(*pair)
            } else {
                u16::from_be_bytes(*pair)
            }
        })
        .collect::<Vec<_>>();
    Ok(String::from_utf16(&units)?)
}

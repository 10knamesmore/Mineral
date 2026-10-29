//! Maps Apple's absolute media-clock TTML and keyed header subtitles into shared lyric lines.

use std::mem::take;

use mineral_model::{LineKind, LyricLine, Lyrics, Word};
use roxmltree::{Document, Node};
use rustc_hash::FxHashMap;

use crate::{Error, Result};

const TTML: &str = "http://www.w3.org/ns/ttml";
const ITUNES: &str = "http://music.apple.com/lyric-ttml-internal";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Timing {
    start_ms: u64,

    end_ms: u64,
}

pub(super) fn parse(
    source: &str,
    translation_language: Option<&str>,
    romanization_script: Option<&str>,
) -> Result<Lyrics> {
    let document = Document::parse(source)?;
    let root = document.root_element();
    if !root.has_tag_name((TTML, "tt")) {
        return Err(Error::InvalidData { field: "TTML root" });
    }
    let translations = secondary(root, "translation", translation_language);
    let romanizations = secondary(root, "transliteration", romanization_script);
    let body = root
        .children()
        .find(|node| node.has_tag_name((TTML, "body")))
        .ok_or(Error::InvalidData { field: "TTML body" })?;
    let mut lines = Vec::new();
    for paragraph in body
        .descendants()
        .filter(|node| node.has_tag_name((TTML, "p")))
    {
        let time_ms = time_attribute(paragraph, "begin")?;
        let end_ms = time_attribute(paragraph, "end")?;
        let mut fragments = Vec::new();
        collect_fragments(paragraph, None, &mut fragments)?;
        let kind = if fragments.iter().any(|(timing, _)| timing.is_some()) {
            let start_ms = time_ms.ok_or(Error::InvalidData {
                field: "TTML line begin",
            })?;
            let end_ms = end_ms.ok_or(Error::InvalidData {
                field: "TTML line end",
            })?;
            let mut words: Vec<Word> = Vec::new();
            let mut leading_text = String::new();
            for (timing, text) in fragments {
                if let Some(timing) = timing {
                    let dur_ms = duration(timing.start_ms, timing.end_ms)?;
                    if let Some(previous) = words.last_mut()
                        && previous.start_ms == timing.start_ms
                        && previous.dur_ms == dur_ms
                    {
                        previous.text.push_str(&text);
                    } else {
                        leading_text.push_str(&text);
                        words.push(Word {
                            start_ms: timing.start_ms,
                            dur_ms,
                            text: take(&mut leading_text),
                        });
                    }
                } else if let Some(previous) = words.last_mut() {
                    // Inter-span spaces belong to the preceding word, not a fabricated time.
                    previous.text.push_str(&text);
                } else {
                    leading_text.push_str(&text);
                }
            }
            words.sort_by_key(|word| word.start_ms);
            LineKind::Words {
                dur_ms: duration(start_ms, end_ms)?,
                words,
            }
        } else {
            if let (Some(start_ms), Some(end_ms)) = (time_ms, end_ms) {
                duration(start_ms, end_ms)?;
            }
            LineKind::Plain(fragments.into_iter().map(|(_, text)| text).collect())
        };
        let key = paragraph.attribute((ITUNES, "key"));
        lines.push(LyricLine {
            time_ms,
            kind,
            translation: key.and_then(|key| translations.get(key).cloned()),
            romanization: key.and_then(|key| romanizations.get(key).cloned()),
        });
    }
    Ok(Lyrics { lines })
}

fn secondary(root: Node<'_, '_>, tag: &str, requested: Option<&str>) -> FxHashMap<String, String> {
    let track = root
        .children()
        .filter(|node| node.has_tag_name((TTML, "head")))
        .flat_map(|head| head.descendants())
        .filter(|node| node.has_tag_name((ITUNES, tag)))
        .filter(|node| tag != "translation" || node.attribute("type") == Some("subtitle"))
        .filter(|node| {
            requested.is_none_or(|requested| {
                node.attribute((XML, "lang"))
                    .is_some_and(|language| language_matches(requested, language))
            })
        })
        .max_by_key(|node| node.attribute((XML, "lang")).map(str::len));
    track
        .into_iter()
        .flat_map(|track| track.children())
        .filter(|node| node.has_tag_name((ITUNES, "text")))
        .filter_map(|node| {
            node.attribute("for")
                .map(|key| (key.to_owned(), text(node)))
        })
        .collect()
}

/// Apple's request tag may include a region absent from the returned xml:lang.
fn language_matches(requested: &str, returned: &str) -> bool {
    requested.eq_ignore_ascii_case(returned)
        || requested
            .get(..returned.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(returned))
            && requested.as_bytes().get(returned.len()) == Some(&b'-')
}

fn text(node: Node<'_, '_>) -> String {
    let mut result = String::new();
    for child in node.descendants() {
        if child.is_text() {
            if let Some(value) = child.text() {
                result.push_str(value);
            }
        } else if child.has_tag_name((TTML, "br")) {
            result.push('\n');
        }
    }
    result
}

fn collect_fragments(
    node: Node<'_, '_>,
    inherited: Option<Timing>,
    fragments: &mut Vec<(Option<Timing>, String)>,
) -> Result<()> {
    let timing = if node.has_tag_name((TTML, "span")) {
        match (time_attribute(node, "begin")?, time_attribute(node, "end")?) {
            (Some(start_ms), Some(end_ms)) => {
                duration(start_ms, end_ms)?;
                Some(Timing { start_ms, end_ms })
            }
            (None, None) => inherited,
            _ => {
                return Err(Error::InvalidData {
                    field: "TTML span timing",
                });
            }
        }
    } else {
        inherited
    };
    if node.is_text() {
        if let Some(value) = node.text() {
            fragments.push((timing, value.to_owned()));
        }
    } else if node.has_tag_name((TTML, "br")) {
        fragments.push((timing, "\n".to_owned()));
    } else {
        for child in node.children() {
            collect_fragments(child, timing, fragments)?;
        }
    }
    Ok(())
}

fn duration(start_ms: u64, end_ms: u64) -> Result<u64> {
    end_ms.checked_sub(start_ms).ok_or(Error::InvalidData {
        field: "TTML time interval",
    })
}

fn time_attribute(node: Node<'_, '_>, name: &str) -> Result<Option<u64>> {
    node.attribute(name).map(timestamp).transpose()
}

/// Clock times (hh:mm:ss.fraction or mm:ss.fraction) and second/millisecond offsets.
/// Fractional precision below one millisecond is truncated using integer arithmetic.
fn timestamp(value: &str) -> Result<u64> {
    let invalid = || Error::InvalidData {
        field: "TTML timestamp",
    };
    if let Some(milliseconds) = value.strip_suffix("ms") {
        return integer(milliseconds).ok_or_else(invalid);
    }
    let clock = value.strip_suffix('s').unwrap_or(value);
    let fields: Vec<&str> = clock.split(':').collect();
    let (seconds, minutes, hours) = match fields.as_slice() {
        [seconds] => (*seconds, None, None),
        [minutes, seconds] => (*seconds, Some(*minutes), None),
        [hours, minutes, seconds] => (*seconds, Some(*minutes), Some(*hours)),
        _ => return Err(invalid()),
    };
    let (whole_seconds, fraction) = match seconds.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (seconds, None),
    };
    let mut seconds = integer(whole_seconds).ok_or_else(invalid)?;
    if let Some(minutes) = minutes {
        let minutes = integer(minutes).ok_or_else(invalid)?;
        if seconds >= 60 || hours.is_some() && minutes >= 60 {
            return Err(invalid());
        }
        seconds = minutes
            .checked_mul(60)
            .and_then(|minutes| minutes.checked_add(seconds))
            .ok_or_else(invalid)?;
    }
    if let Some(hours) = hours {
        seconds = integer(hours)
            .and_then(|hours| hours.checked_mul(3600))
            .and_then(|hours| hours.checked_add(seconds))
            .ok_or_else(invalid)?;
    }
    let fraction_ms = if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let digits = fraction.len().min(3);
        integer(fraction.get(..digits).ok_or_else(invalid)?).ok_or_else(invalid)?
            * 10_u64.pow(u32::try_from(3 - digits)?)
    } else {
        0
    };
    seconds
        .checked_mul(1000)
        .and_then(|seconds| seconds.checked_add(fraction_ms))
        .ok_or_else(invalid)
}

fn integer(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

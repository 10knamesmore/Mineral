//! Parses document lines, preserves untimed text order and resolves word ends after sorting.

use std::borrow::Cow;

use serde::Deserialize;

use crate::lyrics::{LineKind, LyricLine};

use super::enhanced::EnhancedLine;
use super::timestamp::{apply_offset, is_negative_credit_timestamp, parse_timestamp};

/// Parses plain text, LRC, Enhanced LRC and JSON credits into one stable, time-ordered sequence.
///
/// Uses the last valid document offset for LRC times; JSON times are unchanged. Untimed text
/// retains an ordering anchor without acquiring a playback timestamp. Incomplete word timing
/// keeps the line text and its known start. A leading BOM and known metadata are removed.
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut document = Document {
        offset_ms: text.lines().filter_map(parse_offset).next_back(),
        untimed_anchor_ms: None,
        lines: Vec::new(),
    };
    for raw in text.lines() {
        document.parse_line(raw);
    }
    document.finish()
}

/// Owns document-level offset and ordering state while individual lines are parsed.
struct Document<'text> {
    /// Global offset, known before parsing because it may occur at the end of the document.
    offset_ms: Option<i64>,

    /// Sorting position inherited by following untimed text; never copied into its playback time.
    untimed_anchor_ms: Option<u64>,

    /// Lines awaiting stable sorting and the next distinct line time.
    lines: Vec<ParsedLine<'text>>,
}

/// Separates a line's sorting position from its optional playback timestamp.
struct ParsedLine<'text> {
    /// Own timestamp or the inherited anchor for untimed text.
    sort_time_ms: Option<u64>,

    /// Adjusted playback timestamp; absent for plain text or overflowing LRC times.
    time_ms: Option<u64>,

    /// Text or inline segments awaiting their final word boundary.
    content: LineContent<'text>,
}

/// Borrows input text until the final model needs owned strings; JSON text is already owned.
enum LineContent<'text> {
    /// Plain text, including multi-timestamp lines with inline tags removed.
    Text(Cow<'text, str>),

    /// Inline timing belonging to a single line timestamp.
    Enhanced(EnhancedLine<'text>),
}

impl<'text> Document<'text> {
    /// Consumes recognized JSON credits; malformed JSON-shaped text follows the ordinary path.
    fn parse_line(&mut self, raw: &'text str) {
        let raw = raw.trim_start();
        if raw.starts_with('{')
            && let Ok(credit) = serde_json::from_str::<CreditLine>(raw)
        {
            self.push_credit(credit);
            return;
        }
        let prefix = LinePrefix::parse(raw);
        let Some(first_time_ms) = prefix.first_time_ms else {
            if !prefix.text.is_empty() && !is_metadata(prefix.text) {
                self.push_line(None, LineContent::Text(Cow::Borrowed(prefix.text)));
            }
            return;
        };
        let enhanced = EnhancedLine::parse(prefix.text, first_time_ms);
        if prefix.repeated_times_ms.is_empty() {
            let content = match enhanced {
                Some(line) => LineContent::Enhanced(line),
                None => LineContent::Text(Cow::Borrowed(prefix.text)),
            };
            self.push_lrc_line(first_time_ms, content);
        } else {
            // Repeated line timestamps repeat the whole text, not absolute word timestamps.
            let text = match enhanced {
                Some(line) => Cow::Owned(line.plain_text()),
                None => Cow::Borrowed(prefix.text),
            };
            for time_ms in std::iter::once(first_time_ms).chain(prefix.repeated_times_ms) {
                self.push_lrc_line(time_ms, LineContent::Text(text.clone()));
            }
        }
    }

    /// Applies the LRC offset and advances the untimed anchor to the latest LRC time seen.
    fn push_lrc_line(&mut self, raw_time_ms: u64, content: LineContent<'text>) {
        let time_ms = apply_offset(raw_time_ms, self.offset_ms);
        self.push_line(time_ms, content);
        self.untimed_anchor_ms = self.untimed_anchor_ms.max(time_ms);
    }

    /// Joins nonempty JSON credit text and uses its unshifted timestamp as the new anchor.
    fn push_credit(&mut self, credit: CreditLine) {
        let text = credit
            .fragments
            .into_iter()
            .map(|fragment| fragment.text)
            .collect::<String>();
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let time_ms = credit.time_ms.and_then(|time| u64::try_from(time).ok());
        self.push_line(time_ms, LineContent::Text(Cow::Owned(text.to_owned())));
        self.untimed_anchor_ms = time_ms.or(self.untimed_anchor_ms);
    }

    /// Stores untimed text beside its anchor while preserving the absence of playback time.
    fn push_line(&mut self, time_ms: Option<u64>, content: LineContent<'text>) {
        self.lines.push(ParsedLine {
            sort_time_ms: time_ms.or(self.untimed_anchor_ms),
            time_ms,
            content,
        });
    }

    /// Resolves equal-time groups backwards so every group shares the next strictly later time.
    fn finish(mut self) -> Vec<LyricLine> {
        // Unanchored text and zero-time lines share the beginning; stable sorting keeps their order.
        self.lines
            .sort_by_key(|line| line.sort_time_ms.unwrap_or(0));
        let mut lines = Vec::with_capacity(self.lines.len());
        let mut group_time_ms = None;
        let mut next_line_ms = None;
        for parsed in self.lines.into_iter().rev() {
            if let Some(time_ms) = parsed.time_ms
                && group_time_ms != Some(time_ms)
            {
                next_line_ms = group_time_ms;
                group_time_ms = Some(time_ms);
            }
            let kind = match parsed.content {
                LineContent::Text(text) => LineKind::Plain(text.into_owned()),
                LineContent::Enhanced(enhanced) => {
                    enhanced.into_kind(parsed.time_ms, next_line_ms, self.offset_ms)
                }
            };
            lines.push(LyricLine {
                time_ms: parsed.time_ms,
                kind,
                translation: None,
                romanization: None,
            });
        }
        lines.reverse();
        lines
    }
}

/// Keeps the common single timestamp on the stack; repeated timestamps alone allocate a list.
struct LinePrefix<'text> {
    /// First valid leading timestamp, before offset adjustment.
    first_time_ms: Option<u64>,

    /// Further leading timestamps, which each repeat the entire line text.
    repeated_times_ms: Vec<u64>,

    /// Trimmed text after the recognized leading timestamps.
    text: &'text str,
}

impl<'text> LinePrefix<'text> {
    /// Stops at the first ordinary bracket; strips malformed negative-credit times without timing them.
    fn parse(mut text: &'text str) -> Self {
        let mut first_time_ms = None;
        let mut repeated_times_ms = Vec::new();
        while let Some((timestamp, remaining)) =
            text.strip_prefix('[').and_then(|rest| rest.split_once(']'))
        {
            if let Some(time_ms) = parse_timestamp(timestamp) {
                if first_time_ms.is_none() {
                    first_time_ms = Some(time_ms);
                } else {
                    repeated_times_ms.push(time_ms);
                }
            } else if !is_negative_credit_timestamp(timestamp) {
                break;
            }
            text = remaining.trim_start();
        }
        Self {
            first_time_ms,
            repeated_times_ms,
            text: text.trim(),
        }
    }
}

/// Deserializes the existing JSON credit format; negative times leave the text untimed.
#[derive(Deserialize)]
struct CreditLine {
    /// Signed wire timestamp, unaffected by LRC offsets.
    #[serde(default, rename = "t")]
    time_ms: Option<i64>,

    /// Text fragments whose non-text metadata is ignored.
    #[serde(default, rename = "c")]
    fragments: Vec<CreditFragment>,
}

/// Reads text from one credit fragment while leaving links and other metadata out of the model.
#[derive(Deserialize)]
struct CreditFragment {
    /// Fragment text, empty when the field is absent.
    #[serde(default, rename = "tx")]
    text: String,
}

/// Reads a standalone offset line; invalid values leave earlier valid offsets in effect.
fn parse_offset(raw: &str) -> Option<i64> {
    let tag = raw.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (key, value) = tag.split_once(':')?;
    if !key.eq_ignore_ascii_case("offset") {
        return None;
    }
    value.trim().parse().ok()
}

/// Recognizes only known metadata keys, leaving section labels such as `[Verse]` as text.
fn is_metadata(text: &str) -> bool {
    let Some((tag, _)) = text.strip_prefix('[').and_then(|rest| rest.split_once(']')) else {
        return false;
    };
    let Some((key, _)) = tag.split_once(':') else {
        return false;
    };
    [
        "ti", "ar", "al", "au", "by", "offset", "length", "re", "ve", "lang", "kana",
    ]
    .iter()
    .any(|known| key.eq_ignore_ascii_case(known))
}

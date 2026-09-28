//! Converts inline LRC time tags into word durations while borrowing the original text.

use std::ops::Range;

use crate::lyrics::{LineKind, Word};

use super::timestamp::{apply_offset, parse_timestamp};

/// Holds a valid inline timestamp and its UTF-8 byte range, including angle brackets.
struct TimeTag {
    /// Absolute milliseconds before the document offset is applied.
    start_ms: u64,

    /// Byte range removed from the displayed text.
    bytes: Range<usize>,
}

/// Associates text with a start time; an empty slice still marks a gap or an end.
struct Segment<'text> {
    /// Absolute milliseconds, adjusted only when the line is finalized.
    start_ms: u64,

    /// Text between valid tags, with only the whole line's outer whitespace removed.
    text: &'text str,
}

/// Retains borrowed segments until sorted line times can supply a missing final word end.
pub(super) struct EnhancedLine<'text> {
    /// Original line start, used to reject word timestamps that precede their line.
    line_start_ms: u64,

    /// Ordered text segments, including empty timing boundaries.
    segments: Vec<Segment<'text>>,
}

impl<'text> EnhancedLine<'text> {
    /// Splits at valid inline time tags and leaves ordinary angle-bracket text intact.
    pub(super) fn parse(text: &'text str, line_start_ms: u64) -> Option<Self> {
        let mut tags = time_tags(text).peekable();
        let prefix = text.get(..tags.peek()?.bytes.start)?;
        let mut segments = Vec::new();
        if !prefix.trim().is_empty() {
            segments.push(Segment {
                start_ms: line_start_ms,
                text: prefix,
            });
        }
        while let Some(tag) = tags.next() {
            let text_end = tags.peek().map_or(text.len(), |next| next.bytes.start);
            segments.push(Segment {
                start_ms: tag.start_ms,
                text: text.get(tag.bytes.end..text_end)?,
            });
        }
        let mut line = Self {
            line_start_ms,
            segments,
        };
        line.trim_outer_whitespace();
        Some(line)
    }

    /// Removes outer whitespace across empty segments while retaining internal spaces.
    fn trim_outer_whitespace(&mut self) {
        for segment in &mut self.segments {
            segment.text = segment.text.trim_start();
            if !segment.text.is_empty() {
                break;
            }
        }
        for segment in self.segments.iter_mut().rev() {
            segment.text = segment.text.trim_end();
            if !segment.text.is_empty() {
                break;
            }
        }
    }

    /// Joins the text without valid time tags for repeated lines or incomplete word timing.
    pub(super) fn plain_text(&self) -> String {
        self.segments.iter().map(|segment| segment.text).collect()
    }

    /// Builds word timing when every end is known; otherwise retains the complete line text.
    pub(super) fn into_kind(
        mut self,
        line_time_ms: Option<u64>,
        next_line_ms: Option<u64>,
        offset_ms: Option<i64>,
    ) -> LineKind {
        let words = line_time_ms
            .and_then(|start_ms| self.build_word_timing(start_ms, next_line_ms, offset_ms));
        words.unwrap_or_else(|| LineKind::Plain(self.plain_text()))
    }

    /// Validates the original timeline before adjusting boundaries and allocating word text.
    fn build_word_timing(
        &mut self,
        line_time_ms: u64,
        next_line_ms: Option<u64>,
        offset_ms: Option<i64>,
    ) -> Option<LineKind> {
        if self
            .segments
            .iter()
            .any(|segment| segment.start_ms < self.line_start_ms)
            || !self.segments.is_sorted_by_key(|segment| segment.start_ms)
        {
            return None;
        }
        for segment in &mut self.segments {
            segment.start_ms = apply_offset(segment.start_ms, offset_ms)?;
        }
        let last = self.segments.last()?;
        let end_ms = if last.text.is_empty() {
            last.start_ms
        } else {
            next_line_ms?
        };
        let dur_ms = end_ms.checked_sub(line_time_ms)?;
        let mut words = Vec::<Word>::new();
        let ends = self
            .segments
            .iter()
            .skip(1)
            .map(|segment| segment.start_ms)
            .chain(Some(end_ms));
        for (segment, end_ms) in self.segments.iter().zip(ends) {
            if !segment.text.is_empty() {
                words.push(Word {
                    start_ms: segment.start_ms,
                    dur_ms: end_ms.checked_sub(segment.start_ms)?,
                    text: segment.text.to_owned(),
                });
            }
        }
        (!words.is_empty()).then_some(LineKind::Words { dur_ms, words })
    }
}

/// Streams valid tags, pairing each closing bracket with the most recent opening bracket.
fn time_tags(text: &str) -> impl Iterator<Item = TimeTag> {
    let mut opening = None;
    text.char_indices()
        .filter_map(move |(position, character)| match character {
            '<' => {
                opening = Some(position);
                None
            }
            '>' => {
                let start = opening.take()?;
                let timestamp = text.get(start + 1..position)?;
                Some(TimeTag {
                    start_ms: parse_timestamp(timestamp)?,
                    bytes: start..position + 1,
                })
            }
            _ => None,
        })
}

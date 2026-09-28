//! Exports timed lyric text and locates playback within mixed timed and untimed lines.

use crate::lyrics::LyricLine;

/// Serializes timed lines as standard LRC, truncating to centiseconds and omitting untimed text.
/// Returns an empty string when no timed lines exist.
pub fn to_lrc_string(lines: &[LyricLine]) -> String {
    lines
        .iter()
        .filter_map(|line| {
            line.time_ms
                .map(|time_ms| format_lrc_line(time_ms, line.kind.text().as_ref()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Finds the greatest timestamp at or before playback, taking the last line on equal timestamps.
/// Accepts unsorted input and interspersed untimed lines; returns `None` before all timestamps.
pub fn current_line(lines: &[LyricLine], position_ms: u64) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            line.time_ms
                .filter(|time_ms| *time_ms <= position_ms)
                .map(|time_ms| (index, time_ms))
        })
        .max_by_key(|(_, time_ms)| *time_ms)
        .map(|(index, _)| index)
}

/// Reports whether any line can follow a playback timestamp.
pub fn has_timed(lines: &[LyricLine]) -> bool {
    lines.iter().any(|line| line.time_ms.is_some())
}

/// Reports whether any line has word timing.
pub fn has_words(lines: &[LyricLine]) -> bool {
    lines.iter().any(|line| !line.kind.words().is_empty())
}

/// Formats one absolute millisecond time as an LRC line with centisecond precision.
fn format_lrc_line(time_ms: u64, content: &str) -> String {
    let minutes = time_ms / 60_000;
    let seconds = (time_ms % 60_000) / 1_000;
    let centiseconds = (time_ms % 1_000) / 10;
    format!("[{minutes:02}:{seconds:02}.{centiseconds:02}]{content}")
}

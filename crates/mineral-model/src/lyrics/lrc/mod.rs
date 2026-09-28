//! Normalizes plain text, LRC, Enhanced LRC and JSON credits into the shared lyric model.
//! Exports standard line-timed LRC and locates playback within the resulting lines.

mod enhanced;
mod lines;
mod parser;
mod timestamp;

#[cfg(test)]
mod tests;

pub use lines::{current_line, has_timed, has_words, to_lrc_string};
pub use parser::parse_lrc;

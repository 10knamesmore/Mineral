//! 歌词:结构化类型 + 通用 LRC 解析/序列化/定位。

mod lrc;
mod types;

pub use lrc::{current_line, has_timed, has_words, parse_lrc, to_lrc_string};
pub use types::{LineKind, LyricLine, Lyrics, Word};

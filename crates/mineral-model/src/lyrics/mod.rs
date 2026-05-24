//! 歌词:结构化类型 + 通用 LRC 解析/序列化/定位。

mod lrc;
mod types;

pub use types::{LrcLine, LrcLyric, Lyrics, Word, WordLine, WordLyric};

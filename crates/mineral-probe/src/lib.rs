//! 按文件内容探测音频属性与标签(lofty 封装,不信扩展名)。
//!
//! 面向 `Read + Seek` reader,调用方喂本地 `File` 或 storage backend 的读取器皆可。
//! 关键不变量:一律按内容判容器类型(跳 ID3 标签再认帧),不回退文件扩展名。

mod probe;

pub use probe::{ProbedAudio, ProbedTags, file_type_to_format, is_audio_ext, probe};

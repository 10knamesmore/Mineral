//! 下载段(音质 / 目录)。
//!
//! `quality` 直接复用 [`mineral_model::BitRate`](其 serde 已是小写名,契合 schema);
//! `dir` 为 `Option`,Lua `nil`(字段缺省)→ `None`,接线处回落到默认导出目录。

use mineral_config_macros::config_section;
use std::path::PathBuf;

use mineral_model::BitRate;

/// 下载段。
#[config_section]
pub struct DownloadConfig {
    /// 下载音质,与播放音质相互独立。
    quality: BitRate,

    /// 下载导出目录,绝对路径;`None`(Lua `nil`)→ 接线处回落平台默认导出目录(`~/Music/mineral`)。
    dir: Option<PathBuf>,
}

//! 下载段(音质 / 目录)。
//!
//! `quality` 直接复用 [`mineral_model::BitRate`](其 serde 已是小写名,契合 schema);
//! `dir` 为 `Option`,Lua `nil`(字段缺省)→ `None`,接线处回落到默认导出目录。

use std::path::PathBuf;

use mineral_model::BitRate;
use serde::Deserialize;

/// 下载段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DownloadConfig {
    /// 下载音质。
    quality: BitRate,

    /// 下载目录;`None`(Lua `nil`)→ 接线处回落默认导出目录。
    dir: Option<PathBuf>,
}

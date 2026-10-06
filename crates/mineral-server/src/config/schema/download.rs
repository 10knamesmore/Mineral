//! 下载段(音质 / 目录)。
//!
//! `quality` 直接复用 [`mineral_model::BitRate`](其 serde 已是小写名,契合 schema);
//! `dir` 为 `Option`,Lua `nil`(字段缺省)→ `None`,接线处回落到默认导出目录。

use mineral_config_macros::config_section;
use serde::Deserialize;
use std::path::PathBuf;

use mineral_model::BitRate;

/// 下载配置
#[config_section]
pub struct DownloadConfig {
    /// 下载音质
    quality: BitRate,

    /// 下载绝对路径；省略用 ~/Music/mineral
    dir: Option<PathBuf>,

    /// 下载并发数，至少 1
    #[serde(deserialize_with = "deserialize_positive_usize")]
    max_concurrent: usize,

    /// 为下载和缓存写入音频标签
    tagging: bool,

    /// 打标并发数；0 或 1 为串行
    tagging_workers: usize,
}

/// Deserializes a strictly positive concurrency limit.
fn deserialize_positive_usize<'de, D>(deserializer: D) -> std::result::Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    if value == 0 {
        Err(serde::de::Error::custom("must be at least 1"))
    } else {
        Ok(value)
    }
}

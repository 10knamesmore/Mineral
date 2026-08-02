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

    /// 落盘(下载导出 / 播放缓存)后给音频文件内嵌 metadata tag(标题 / 艺人 / 专辑 / 封面 /
    /// 歌词等);后台打标,失败只记日志,不影响下载与播放。
    tagging: bool,

    /// 打标并发 worker 数(同时也是对源站请求并发的放大系数;`0` / `1` = 串行)。
    tagging_workers: usize,
}

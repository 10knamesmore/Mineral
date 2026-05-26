//! 上报给系统媒体控件的播放状态与曲目元数据。

use std::time::Duration;

use mineral_model::{LrcLyric, WordLyric};
use typed_builder::TypedBuilder;

/// 播放状态(上报给系统媒体控件)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    /// 正在播放。
    Playing,

    /// 已暂停。
    Paused,

    /// 已停止(无当前曲目)。
    Stopped,
}

/// 当前曲目元数据,上报给系统媒体控件显示。
///
/// 字段私有 + builder 构造,避免外部用字面量直接拼。所有字段可选:拿不到的
/// 信息留空即可。
///
/// 各字段是否被读取随平台后端而定:Linux 后端把全部字段映射进 metadata;非 Linux
/// 后端只用其中一部分(封面走另一条字节通道、扩展歌词轨在系统媒体中心无对应面)。
/// 故对非 Linux 平台放开未读字段的 `dead_code`,避免按平台裁剪 builder API。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug, Default, TypedBuilder)]
#[non_exhaustive]
pub struct NowPlaying {
    /// 曲目标题。
    #[builder(default)]
    pub(crate) title: Option<String>,

    /// 艺人(多位艺人由调用方拼成单串传入)。
    #[builder(default)]
    pub(crate) artist: Option<String>,

    /// 专辑名。
    #[builder(default)]
    pub(crate) album: Option<String>,

    /// 封面 URL(`http(s)://` 或 `file://`)。
    #[builder(default)]
    pub(crate) cover_url: Option<String>,

    /// 曲目总时长。
    #[builder(default)]
    pub(crate) duration: Option<Duration>,

    /// 行级原文歌词。空 = 无。序列化成标准 LRC 透传到 MPRIS `xesam:asText`,
    /// 标准客户端可直接读它配合 position 做逐行同步高亮。
    #[builder(default)]
    pub(crate) lrc: LrcLyric,

    /// 逐字原文歌词。空 = 无。序列化成 JSON 透传到自定义 key `mineral:words`,
    /// 可编程显示端(如 quickshell)解析后做逐字 wipe 高亮;缺失时回退 `xesam:asText`。
    #[builder(default)]
    pub(crate) words: WordLyric,

    /// 行级翻译。空 = 无。序列化成标准 LRC 透传到 `mineral:translation`,
    /// 与原文共享时间轴做双行展示。
    #[builder(default)]
    pub(crate) translation: LrcLyric,

    /// 行级罗马音。空 = 无。序列化成标准 LRC 透传到 `mineral:romanization`。
    #[builder(default)]
    pub(crate) romanization: LrcLyric,
}

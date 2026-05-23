//! 上报给系统媒体控件的播放状态与曲目元数据。

use std::time::Duration;

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

    /// 完整 LRC 歌词文本(带 `[mm:ss.xx]` 时间戳),透传到 MPRIS `xesam:asText`。
    /// 可编程显示端(如 quickshell)读它解析 + 配合 position 做逐行同步高亮。
    #[builder(default)]
    pub(crate) lyrics: Option<String>,
}

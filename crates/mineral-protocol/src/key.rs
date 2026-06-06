//! 按键瞬间的 client 上下文快照 — [`KeyContext`] / [`ViewKind`]。
//!
//! TUI 触发脚本动作(`Request::InvokeAction`)时随手采集,让 Lua 回调拿到
//! 「按键那一刻用户在看什么 / 选中什么」;CLI 等无界面触发面传 `None`。

use mineral_model::{PlaylistId, SongId};
use serde::{Deserialize, Serialize};

/// 按键瞬间 client 正在展示的视图。
///
/// 与 TUI 内部视图枚举解耦:这里是跨 client 的语义归类,TUI 在采集时做映射。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewKind {
    /// 歌单列表。
    Playlists,

    /// 曲目列表(library / 歌单详情)。
    Tracks,

    /// 播放队列。
    Queue,

    /// 全屏播放页。
    Fullscreen,

    /// 搜索态。
    Search,
}

impl ViewKind {
    /// 蛇形稳定名(Lua ctx 表里 `view` 字段的取值)。
    #[must_use]
    pub fn script_name(self) -> &'static str {
        match self {
            Self::Playlists => "playlists",
            Self::Tracks => "tracks",
            Self::Queue => "queue",
            Self::Fullscreen => "fullscreen",
            Self::Search => "search",
        }
    }
}

/// 按键瞬间的上下文快照(只读采集,不含任何可变状态)。
///
/// 私有字段 + builder 构造 + getter 读取;加字段经 builder 是兼容增量。
#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    typed_builder::TypedBuilder,
    derive_getters::Getters,
)]
#[non_exhaustive]
pub struct KeyContext {
    /// 按键时所在视图。
    view: ViewKind,

    /// 列表光标选中的歌(无选中 / 视图无歌列表为 `None`)。
    selected_song_id: Option<SongId>,

    /// 列表光标选中的歌单(非歌单视图为 `None`)。
    selected_playlist_id: Option<PlaylistId>,

    /// 在播的歌(停止态为 `None`)。
    now_playing_id: Option<SongId>,
}

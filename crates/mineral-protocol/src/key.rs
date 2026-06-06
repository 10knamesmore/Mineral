//! 按键瞬间的 client 上下文快照 — [`KeyContext`] / [`ViewKind`]。
//!
//! TUI 触发脚本动作(`Request::InvokeAction`)时随手采集,让 Lua 回调拿到
//! 「按键那一刻用户在看什么 / 选中什么」;CLI 等无界面触发面传 `None`。

use mineral_model::{PlaylistId, Song};
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

/// 脚本 `mineral.bind` 产生的一条键绑定(daemon → client 下发,client
/// 解析 key 字符串合进自己的 keymap)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptBind {
    /// 键字符串(`KeyChord` nvim 文法,如 `"X"` / `"<C-g>"`;解析在 client 侧)。
    pub key: String,

    /// 触发的动作注册名(bind 生成的内部名,如 `"bind#1"`)。
    pub action: String,
}

/// 歌单的轻量引用(id + 展示名;曲目不随上下文传输)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaylistRef {
    /// 歌单 id。
    pub id: PlaylistId,

    /// 歌单名。
    pub name: String,
}

/// 按键瞬间的上下文快照(只读采集,不含任何可变状态)。
///
/// 职责边界:只携带 **daemon 不知道的 client 侧信息**(在看什么 / 选中什么 /
/// 搜索词);播放器态(音量 / 进度 / 队列)归属性树 `mineral.get`,不在此重复。
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

    /// 列表光标选中的歌(无选中 / 视图无歌列表为 `None`;队列浮层取光标条目)。
    selected_song: Option<Box<Song>>,

    /// 列表光标选中 / 所在的歌单(非歌单语境为 `None`)。
    selected_playlist: Option<PlaylistRef>,

    /// 在播的歌(停止态为 `None`)。
    now_playing: Option<Box<Song>>,

    /// 选中歌的 ♥ 态(client 侧装饰缓存;无选中 / 未知为 `None`)。
    selected_loved: Option<bool>,

    /// 当前搜索 / 过滤词(空词为 `None`;搜索输入态与过滤残留态都给)。
    search_query: Option<String>,
}

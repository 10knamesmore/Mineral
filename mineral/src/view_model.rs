//! 渲染层共享的 UI 装饰类型 — 不依赖任何具体 channel。
//!
//! 真实 channel 接入时,把 `mineral_model::Playlist` / `Song` 包装成
//! [`PlaylistView`] / [`SongView`],额外字段(`kind` / `loved` / `plays`)
//! 由具体 channel 提供;不知道时给 `None` / 默认值。

use mineral_model::{Playlist, Song};

/// 歌单类别(对应设计稿 ★/◆/#/♪ 字形)。具体 channel 决定如何分类自己的歌单。
#[allow(dead_code)] // reason: 变体仅在启用 mock feature 时由 mineral-channel-mock 构造
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistKind {
    /// 系统(★)。
    System,
    /// 智能(◆)。
    Smart,
    /// 流派(#)。
    Genre,
    /// 用户(♪)。
    User,
}

impl PlaylistKind {
    /// 类型字形。
    pub fn glyph(self) -> &'static str {
        match self {
            Self::System => "★",
            Self::Smart => "◆",
            Self::Genre => "#",
            Self::User => "♪",
        }
    }
}

/// 一条歌单 + UI 装饰(kind 可缺省)。
#[derive(Clone, Debug)]
pub struct PlaylistView {
    /// 底层 model。
    pub data: Playlist,
    /// 已知的歌单类别。channel 不区分 / 不提供时给 `None`。
    pub kind: Option<PlaylistKind>,
}

impl PlaylistView {
    /// 该歌单内全部曲目时长之和(ms)。
    pub fn total_duration_ms(&self) -> u64 {
        self.data.songs.iter().map(|s| s.duration_ms).sum()
    }
}

/// 一首歌 + UI 装饰(`loved` / `plays`),channel 不提供时给默认值
/// (`false` / `0`)。
#[derive(Clone, Debug)]
pub struct SongView {
    /// 底层 model。
    pub data: Song,
    /// 是否已收藏。
    pub loved: bool,
    /// 累计播放次数。
    pub plays: u32,
}

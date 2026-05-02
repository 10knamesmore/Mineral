//! 渲染层共享的 UI 装饰类型 — 不依赖任何具体 channel。
//!
//! 真实 channel 接入时,把 `mineral_model::Playlist` / `Song` 包装成
//! [`PlaylistView`] / [`SongView`],额外字段(`loved` / `plays`)由具体
//! channel 提供;不知道时给默认值。

use mineral_model::{Playlist, Song};

/// 一条歌单 + UI 装饰。
#[derive(Clone, Debug)]
pub struct PlaylistView {
    /// 底层 model。
    pub data: Playlist,
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

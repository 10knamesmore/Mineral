//! 渲染层共享的 UI 装饰类型 — 不依赖任何具体 channel。
//!
//! 真实 channel 接入时,把 `mineral_model::Playlist` / `PlaylistEntry` 包装成
//! [`PlaylistView`] / [`PlaylistEntryView`]；额外字段(`loved` / `plays`)由 user-data 装饰。

use mineral_model::{Playlist, PlaylistEntry};

/// 一条歌单 + UI 装饰。
#[derive(Clone, Debug)]
pub struct PlaylistView {
    /// 底层 model。
    pub data: Playlist,
}

/// 一条 Playlist membership + Song UI 装饰。
///
/// Relation 保留 authoritative CollectionIndex；non-collection Song 不伪造 membership。
#[derive(Clone, Debug)]
pub struct PlaylistEntryView {
    /// 底层 model relation。
    pub data: PlaylistEntry,

    /// 该 relation 指向的 Song 是否已收藏。
    pub loved: bool,

    /// 该 relation 指向的 Song 的远端真实累计播放次数。
    pub plays: Option<u32>,
}

//! [`CancelFilter`] — IPC 化的批量取消条件。
//!
//! 闭包过不了 wire,但「按种类砍一批」这种谓词足以覆盖现有所有 cancel 场景。
//! 用 enum + tag list 表达。

use mineral_task::{ChannelFetchKind, TaskKind};
use serde::{Deserialize, Serialize};

/// IPC-friendly 的批量取消条件。enum + Vec<tag>,可序列化。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CancelFilter {
    /// 取消所有 [`TaskKind::ChannelFetch`] 任务,且其 [`ChannelFetchKind`] 命中给定 tag。
    /// 空 vec 等价 no-op。
    ChannelFetchKinds(Vec<ChannelFetchKindTag>),
}

impl CancelFilter {
    /// 给定一个具体 [`TaskKind`],判断本 filter 是否要砍。
    #[must_use]
    pub fn matches(&self, kind: &TaskKind) -> bool {
        match (self, kind) {
            (Self::ChannelFetchKinds(tags), TaskKind::ChannelFetch(k)) => {
                tags.contains(&ChannelFetchKindTag::of(k))
            }
        }
    }
}

/// [`ChannelFetchKind`] 的 wire-friendly 标签。本身不带任何字段——只用于「按种类砍一批」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelFetchKindTag {
    /// 对应 [`ChannelFetchKind::MyPlaylists`]。
    MyPlaylists,
    /// 对应 [`ChannelFetchKind::LikedSongIds`]。
    LikedSongIds,
    /// 对应 [`ChannelFetchKind::PlaylistTracks`]。
    PlaylistTracks,
    /// 对应 [`ChannelFetchKind::SongUrl`]。
    SongUrl,
    /// 对应 [`ChannelFetchKind::Lyrics`]。
    Lyrics,
}

impl ChannelFetchKindTag {
    /// 取一个具体 [`ChannelFetchKind`] 的标签。
    #[must_use]
    pub fn of(kind: &ChannelFetchKind) -> Self {
        match kind {
            ChannelFetchKind::MyPlaylists { .. } => Self::MyPlaylists,
            ChannelFetchKind::LikedSongIds { .. } => Self::LikedSongIds,
            ChannelFetchKind::PlaylistTracks { .. } => Self::PlaylistTracks,
            ChannelFetchKind::SongUrl { .. } => Self::SongUrl,
            ChannelFetchKind::Lyrics { .. } => Self::Lyrics,
        }
    }
}

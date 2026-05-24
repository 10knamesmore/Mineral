//! 任务种类与 dedup 键。

use mineral_model::{PlaylistId, SongId, SourceKind};
use serde::{Deserialize, Serialize};

use crate::lane::Lane;

/// 一个待调度任务的所有信息。具体业务参数挂在子枚举里。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskKind {
    /// channel 数据拉取。
    ChannelFetch(ChannelFetchKind),
    // 后续:Search / PlayPrep / AuthRefresh / PrePreload / LocalScan
}

impl TaskKind {
    /// 该任务路由到的 lane。
    pub fn lane(&self) -> Lane {
        match self {
            Self::ChannelFetch(_) => Lane::ChannelFetch,
        }
    }

    /// 用于 ongoing 去重:相同 key 的任务在 ongoing 里只保留一条;后到的命中既存任务。
    pub fn dedup_key(&self) -> DedupKey {
        match self {
            Self::ChannelFetch(k) => DedupKey(format!("ChannelFetch:{}", k.dedup_part())),
        }
    }
}

/// channel 数据拉取的具体形态。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelFetchKind {
    /// 拉某 channel 当前用户的歌单列表。
    MyPlaylists {
        /// 目标 channel。
        source: SourceKind,
    },

    /// 拉某 channel 当前用户喜欢的歌曲 ID 集合(♥ 装饰用)。
    LikedSongIds {
        /// 目标 channel。
        source: SourceKind,
    },

    /// 拉某歌单内的曲目(目标 channel 由 `id` 的 namespace 决定)。
    PlaylistTracks {
        /// 歌单 id(自带 namespace)。
        id: PlaylistId,
    },

    /// 解析某首歌的播放 URL(用于 PlayPrep;目标 channel 由 `song_id` 的 namespace 决定)。
    SongUrl {
        /// 歌曲 id(自带 namespace)。
        song_id: SongId,
    },

    /// 拉某首歌的歌词(目标 channel 由 `song_id` 的 namespace 决定)。
    Lyrics {
        /// 歌曲 id(自带 namespace)。
        song_id: SongId,
    },
}

impl ChannelFetchKind {
    /// 产出 dedup key 的可变部分(channel + 子操作 + 关键参数)。
    ///
    /// 带 id 的形态用 `id.qualified()`(namespace 已在 id 内,自然带上来源);
    /// 只有 source 的形态(无 id 可派生)仍显式拼 `{source:?}`。
    fn dedup_part(&self) -> String {
        match self {
            Self::MyPlaylists { source } => format!("{source:?}:my_playlists"),
            Self::LikedSongIds { source } => format!("{source:?}:liked_song_ids"),
            Self::PlaylistTracks { id } => format!("playlist_tracks:{}", id.qualified()),
            Self::SongUrl { song_id } => format!("song_url:{}", song_id.qualified()),
            Self::Lyrics { song_id } => format!("lyrics:{}", song_id.qualified()),
        }
    }

    /// 该任务针对的 channel(给 lane 路由 worker 用)。
    ///
    /// 带 id 的形态从 id 的 namespace 派生;只有 source 的形态直接返回。
    pub fn source(&self) -> SourceKind {
        match self {
            Self::MyPlaylists { source } | Self::LikedSongIds { source } => *source,
            Self::PlaylistTracks { id } => id.namespace(),
            Self::SongUrl { song_id } | Self::Lyrics { song_id } => song_id.namespace(),
        }
    }
}

/// [`ChannelFetchKind`] 的 wire-friendly 标签:无字段 enum,可哈希、可序列化。
///
/// 用于:跨进程「按种类砍一批」(见 `mineral_protocol::CancelFilter`)、按 kind 计数
/// (见 [`crate::Snapshot::by_kind`])。
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

/// 任务去重键(`Eq + Hash` 安全)。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DedupKey(String);

impl std::fmt::Display for DedupKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

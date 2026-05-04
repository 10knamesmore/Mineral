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

    /// 拉某 channel 某歌单内的曲目。
    PlaylistTracks {
        /// 目标 channel。
        source: SourceKind,

        /// 歌单 id。
        id: PlaylistId,
    },

    /// 解析某首歌在指定 channel 下的播放 URL(用于 PlayPrep)。
    SongUrl {
        /// 目标 channel。
        source: SourceKind,

        /// 歌曲 id。
        song_id: SongId,
    },

    /// 拉某首歌的歌词。
    Lyrics {
        /// 目标 channel。
        source: SourceKind,

        /// 歌曲 id。
        song_id: SongId,
    },
}

impl ChannelFetchKind {
    fn dedup_part(&self) -> String {
        match self {
            Self::MyPlaylists { source } => format!("{source:?}:my_playlists"),
            Self::LikedSongIds { source } => format!("{source:?}:liked_song_ids"),
            Self::PlaylistTracks { source, id } => {
                format!("{source:?}:playlist_tracks:{}", id.as_str())
            }
            Self::SongUrl { source, song_id } => {
                format!("{source:?}:song_url:{}", song_id.as_str())
            }
            Self::Lyrics { source, song_id } => {
                format!("{source:?}:lyrics:{}", song_id.as_str())
            }
        }
    }

    /// 该任务针对的 channel(给 lane 路由 worker 用)。
    pub fn source(&self) -> SourceKind {
        match self {
            Self::MyPlaylists { source }
            | Self::LikedSongIds { source }
            | Self::PlaylistTracks { source, .. }
            | Self::SongUrl { source, .. }
            | Self::Lyrics { source, .. } => *source,
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

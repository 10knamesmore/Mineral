//! 任务种类与 dedup 键。

use mineral_model::{BitRate, PlaylistId, SongId, SourceKind};
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

        /// 期望音质(channel 据此选流;无权限时由 channel 内部降级)。
        quality: BitRate,
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
            Self::SongUrl { song_id, quality } => {
                format!("song_url:{}:{quality:?}", song_id.qualified())
            }
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
            Self::SongUrl { song_id, .. } | Self::Lyrics { song_id } => song_id.namespace(),
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest::sample::select;

    use mineral_model::{BitRate, SongId, SourceKind};

    use super::{ChannelFetchKind, TaskKind};

    /// 任意 `BitRate` 变体。
    fn arb_bitrate() -> impl Strategy<Value = BitRate> {
        select(vec![
            BitRate::Standard,
            BitRate::Higher,
            BitRate::Exhigh,
            BitRate::Lossless,
            BitRate::Hires,
        ])
    }

    /// 构造 `SongUrl` 任务(裸 id 串自带 NETEASE namespace)。
    fn song_url(raw: &str, quality: BitRate) -> TaskKind {
        TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
            song_id: SongId::new(SourceKind::NETEASE, raw),
            quality,
        })
    }

    proptest! {
        /// 同一首歌的 `SongUrl` dedup key **当且仅当** 音质相同才相等——不同音质要能各自
        /// 入队(否则切换音质会被误去重吞掉),同音质必须合并。
        #[test]
        fn song_url_dedup_keyed_by_quality(
            raw in "[0-9]{1,10}",
            q1 in arb_bitrate(),
            q2 in arb_bitrate(),
        ) {
            let same_quality = q1 == q2;
            let keys_equal = song_url(&raw, q1).dedup_key() == song_url(&raw, q2).dedup_key();
            prop_assert_eq!(keys_equal, same_quality);
        }

        /// 不同歌曲的 `SongUrl` 永远不同 key(即便音质相同)。
        #[test]
        fn distinct_songs_distinct_keys(
            r1 in "[0-9]{1,10}",
            r2 in "[0-9]{1,10}",
            q in arb_bitrate(),
        ) {
            prop_assume!(r1 != r2);
            prop_assert_ne!(song_url(&r1, q).dedup_key(), song_url(&r2, q).dedup_key());
        }
    }
}

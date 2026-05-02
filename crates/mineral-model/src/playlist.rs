use serde::{Deserialize, Serialize};

use crate::{ids::PlaylistId, song::Song, source::SourceKind, url::MediaUrl};

/// 一个歌单及其曲目。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    /// 来源 channel。
    pub source: SourceKind,
    /// 歌单 ID(在 `source` 内唯一)。
    pub id: PlaylistId,
    /// 歌单名。
    pub name: String,
    /// 简介,拿不到给空。
    pub description: String,
    /// 封面 URL。
    pub cover_url: Option<MediaUrl>,
    /// 标称曲目数(可能与 `songs.len()` 不一致——分页或仅头部加载时)。
    pub track_count: u64,
    /// 已加载的曲目列表。
    pub songs: Vec<Song>,
}

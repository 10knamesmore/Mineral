use serde::{Deserialize, Serialize};

use crate::{ids::AlbumId, refs::ArtistRef, song::Song, source::SourceKind, url::MediaUrl};

/// 一张专辑及其曲目。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    /// 来源 channel。
    pub source: SourceKind,
    /// 专辑 ID(在 `source` 内唯一)。
    pub id: AlbumId,
    /// 专辑名。
    pub name: String,
    /// 关联艺人(主艺人在前)。
    pub artists: Vec<ArtistRef>,
    /// 简介,拿不到给空。
    pub description: String,
    /// 发行时间(Unix epoch ms)。
    pub publish_time_ms: i64,
    /// 封面 URL,无封面给 `None`。
    pub cover_url: Option<MediaUrl>,
    /// 曲目列表,顺序 = 专辑曲目顺序。
    pub songs: Vec<Song>,
}

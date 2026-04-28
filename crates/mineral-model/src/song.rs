use serde::{Deserialize, Serialize};

use crate::{
    ids::SongId,
    refs::{AlbumRef, ArtistRef},
    source::SourceKind,
    url::MediaUrl,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Song {
    pub source: SourceKind,
    pub id: SongId,
    pub name: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration_ms: u64,
    /// 封面图。远端 channel 通常给 `Remote(http(s)://...)`,
    /// 本地源若有内嵌封面可以给 `Local(...)` 指向缓存出来的文件。
    pub cover_url: Option<MediaUrl>,
    /// 这首歌的"原始位置"——本地源就是音频文件路径(`Local`);
    /// 远端源若已下载到缓存可以填 `Local`,否则为 `None`,需走 `song_urls`。
    pub source_url: Option<MediaUrl>,
}

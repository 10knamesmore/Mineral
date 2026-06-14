use derive_getters::Getters;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use crate::{
    ids::SongId,
    refs::{AlbumRef, ArtistRef},
    source::SourceKind,
    url::MediaUrl,
};

/// 一首歌曲的核心元数据。
///
/// 构造走 [`Song::builder`](Song::builder)(`#[non_exhaustive]`:新增字段不破坏外部构造);
/// 读取走 getter。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct Song {
    /// 歌曲 ID(自带 namespace,在其来源内唯一)。
    pub id: SongId,

    /// 歌名。
    pub name: String,

    /// 译名(原名的翻译,如外文曲名的中文译名),拿不到给 `None`。
    #[builder(default)]
    pub translation: Option<String>,

    /// 关联艺人(主艺人在前)。
    #[builder(default)]
    pub artists: Vec<ArtistRef>,

    /// 所属专辑(单曲为 `None`)。
    #[builder(default)]
    pub album: Option<AlbumRef>,

    /// 时长(ms),拿不到给 0。
    #[builder(default)]
    pub duration_ms: u64,

    /// 封面图。远端 channel 通常给 `Remote(http(s)://...)`,
    /// 本地源若有内嵌封面可以给 `Local(...)` 指向缓存出来的文件。
    #[builder(default)]
    pub cover_url: Option<MediaUrl>,

    /// 这首歌的"原始位置"——本地源就是音频文件路径(`Local`);
    /// 远端源若已下载到缓存可以填 `Local`,否则为 `None`,需走 `song_urls`。
    #[builder(default)]
    pub source_url: Option<MediaUrl>,
}

impl Song {
    /// 来源 channel——派生自 [`Song::id`] 的 namespace。
    #[inline]
    pub fn source(&self) -> SourceKind {
        self.id.namespace()
    }
}

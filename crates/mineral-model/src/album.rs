use derive_getters::Getters;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use crate::{
    collection::AlbumTrack, ids::AlbumId, refs::ArtistRef, source::SourceKind, url::MediaUrl,
};

/// 一张专辑及其曲目。
///
/// 构造走 [`Album::builder`](Album::builder)(`#[non_exhaustive]`);读取走 getter。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct Album {
    /// 专辑 ID(自带 namespace,在其来源内唯一)。
    pub id: AlbumId,

    /// 专辑名。
    pub name: String,

    /// 关联艺人(主艺人在前)。
    #[builder(default)]
    pub artists: Vec<ArtistRef>,

    /// 简介,拿不到给空。
    #[builder(default)]
    pub description: String,

    /// 发行方(唱片公司 / 厂牌)。
    #[builder(default)]
    pub company: Option<String>,

    /// 发行时间(Unix epoch ms)。
    #[builder(default)]
    pub publish_time_ms: i64,

    /// album 曲目总数;`None` = **未知**(搜索 / 投稿列表等轻量投影拿不到,须下钻 `album_detail`
    /// 才确定)——与「真的 0 曲」区分开,展示层据此画 `-` 而非 `0`。与 `tracks` 是否已填充无关。
    #[builder(default)]
    pub track_count: Option<u64>,

    /// 封面 URL,无封面给 `None`。
    #[builder(default)]
    pub cover_url: Option<MediaUrl>,

    /// 已加载的曲目 relation；每项保留 canonical snapshot 中的 0-based absolute index。
    #[builder(default)]
    pub tracks: Vec<AlbumTrack>,
}

impl Album {
    /// 来源(source)——派生自 [`Album::id`] 的 namespace。
    #[inline]
    pub fn source(&self) -> SourceKind {
        self.id.namespace()
    }
}

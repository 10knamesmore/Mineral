use derive_getters::Getters;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use crate::{ids::ArtistId, song::Song, source::SourceKind, url::MediaUrl};

/// 艺人及其代表曲目。
///
/// 构造走 [`Artist::builder`](Artist::builder)(`#[non_exhaustive]`);读取走 getter。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct Artist {
    /// 艺人 ID(自带 namespace,在其来源内唯一)。
    pub id: ArtistId,

    /// 艺名。
    pub name: String,

    /// 简介,拿不到给空。
    #[builder(default)]
    pub description: String,

    /// 关注者/粉丝数;`None` = **未知**(接口没给)——与「真的 0 粉丝」区分开,
    /// 展示层据此画占位而非 `0`。
    #[builder(default)]
    pub follower_count: Option<u64>,

    /// 名下 album 总数,拿不到给 `None`。
    #[builder(default)]
    pub album_count: Option<u64>,

    /// 名下歌曲总数,拿不到给 `None`。
    #[builder(default)]
    pub song_count: Option<u64>,

    /// 头像 URL。
    #[builder(default)]
    pub avatar_url: Option<MediaUrl>,

    /// 代表 / 热门曲目。
    #[builder(default)]
    pub songs: Vec<Song>,
}

impl Artist {
    /// 来源(source)——派生自 [`Artist::id`] 的 namespace。
    #[inline]
    pub fn source(&self) -> SourceKind {
        self.id.namespace()
    }
}

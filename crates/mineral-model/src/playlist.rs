use derive_getters::Getters;
use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use crate::{ids::PlaylistId, song::Song, source::SourceKind, url::MediaUrl};

/// 一个歌单及其曲目。
///
/// 构造走 [`Playlist::builder`](Playlist::builder)(`#[non_exhaustive]`);读取走 getter。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct Playlist {
    /// 歌单 ID(自带 namespace,在其来源内唯一)。
    pub id: PlaylistId,

    /// 歌单名。
    pub name: String,

    /// 简介,拿不到给空。
    #[builder(default)]
    pub description: String,

    /// 封面 URL。
    #[builder(default)]
    pub cover_url: Option<MediaUrl>,

    /// 标称曲目数(可能与 `songs.len()` 不一致——分页或仅头部加载时)。
    #[builder(default)]
    pub track_count: u64,

    /// 播放量,拿不到给 `None`。
    #[builder(default)]
    pub play_count: Option<u64>,

    /// 收藏 / 订阅数(多少用户收藏了此 playlist),拿不到给 `None`。
    #[builder(default)]
    pub subscriber_count: Option<u64>,

    /// 已加载的曲目列表。
    #[builder(default)]
    pub songs: Vec<Song>,
}

impl Playlist {
    /// 来源(source)——派生自 [`Playlist::id`] 的 namespace。
    #[inline]
    pub fn source(&self) -> SourceKind {
        self.id.namespace()
    }
}

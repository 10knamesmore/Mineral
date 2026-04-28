use serde::{Deserialize, Serialize};

use crate::{ids::ArtistId, song::Song, source::SourceKind, url::MediaUrl};

/// 艺人及其代表曲目。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    /// 来源 channel。
    pub source: SourceKind,
    /// 艺人 ID(在 `source` 内唯一)。
    pub id: ArtistId,
    /// 艺名。
    pub name: String,
    /// 简介,拿不到给空。
    pub description: String,
    /// 关注者/粉丝数,拿不到给 0。
    pub follower_count: u64,
    /// 头像 URL。
    pub avatar_url: Option<MediaUrl>,
    /// 代表 / 热门曲目。
    pub songs: Vec<Song>,
}

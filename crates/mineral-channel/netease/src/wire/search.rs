//! 搜索端点的响应结构。

use serde::Deserialize;

use super::song::{Artist, SearchSong};

/// `/weapi/search/get` type=1（单曲）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchSongsResult {
    /// 命中的歌曲列表。
    #[serde(default)]
    pub songs: Vec<SearchSong>,
}

/// `/weapi/search/get` type=10（专辑）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchAlbumsResult {
    /// 命中的专辑列表。
    #[serde(default)]
    pub albums: Vec<SearchAlbum>,
}

/// 搜索结果里出现的专辑（结构与 song 子模块下的 `Album` 不同：多了 description 等）。
#[derive(Debug, Deserialize)]
pub struct SearchAlbum {
    /// 专辑 ID。
    pub id: i64,

    /// 专辑名。
    #[serde(default)]
    pub name: String,

    /// 主艺术家。
    #[serde(default)]
    pub artist: Option<Artist>,

    /// 描述。
    #[serde(default)]
    pub description: String,

    /// 发行时间（毫秒时间戳）。
    #[serde(default, rename = "publishTime")]
    pub publish_time: i64,

    /// 封面 URL。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,
}

/// `/weapi/search/get` type=1000（歌单）的响应。
#[derive(Debug, Deserialize)]
pub struct SearchPlaylistsResult {
    /// 命中的歌单列表。
    #[serde(default)]
    pub playlists: Vec<SearchPlaylist>,
}

/// 搜索结果里出现的歌单。
#[derive(Debug, Deserialize)]
pub struct SearchPlaylist {
    /// 歌单 ID。
    pub id: i64,

    /// 歌单名。
    #[serde(default)]
    pub name: String,

    /// 歌单描述。
    #[serde(default)]
    pub description: Option<String>,

    /// 歌单封面 URL。
    #[serde(default, rename = "coverImgUrl")]
    pub cover_img_url: Option<String>,

    /// 歌单内曲目数。
    #[serde(default, rename = "trackCount")]
    pub track_count: u64,
}

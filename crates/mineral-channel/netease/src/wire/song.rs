//! 歌曲、艺术家、专辑相关的协议结构。

use serde::Deserialize;

/// 协议层艺术家结构（出现在搜索结果、歌曲详情等多个端点）。
#[derive(Debug, Clone, Deserialize)]
pub struct Artist {
    /// 网易云艺术家数字 ID。
    pub id: i64,

    /// 艺术家名。
    #[serde(default)]
    pub name: String,
}

/// 协议层专辑结构。
#[derive(Debug, Clone, Deserialize)]
pub struct Album {
    /// 网易云专辑数字 ID。
    pub id: i64,

    /// 专辑名。
    #[serde(default)]
    pub name: String,

    /// 封面 URL（部分端点会缺）。
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,
}

/// 搜索结果里出现的歌曲（用 `artists`、`album`、`duration` 字段）。
#[derive(Debug, Clone, Deserialize)]
pub struct SearchSong {
    /// 歌曲 ID。
    pub id: i64,

    /// 歌曲名。
    #[serde(default)]
    pub name: String,

    /// 艺术家列表。
    #[serde(default)]
    pub artists: Vec<Artist>,

    /// 专辑信息。
    pub album: Album,

    /// 时长（毫秒）。
    #[serde(default)]
    pub duration: u64,
}

/// 专辑/歌单详情里出现的歌曲（用 `ar`、`al`、`dt` 字段）。
#[derive(Debug, Clone, Deserialize)]
pub struct AlbumSong {
    /// 歌曲 ID。
    pub id: i64,

    /// 歌曲名。
    #[serde(default)]
    pub name: String,

    /// 艺术家列表（网易云在专辑/歌单 detail 端点用 `ar` 字段名）。
    #[serde(default)]
    pub ar: Vec<Artist>,

    /// 专辑信息（同上，端点字段名为 `al`）。
    pub al: Album,

    /// 时长（毫秒，字段名为 `dt`）。
    #[serde(default)]
    pub dt: u64,
}

/// 播放 URL 端点的单首歌响应。
#[derive(Debug, Clone, Deserialize)]
pub struct SongUrl {
    /// 歌曲 ID。
    pub id: i64,

    /// 实际可用的播放 URL（试听片段或拒绝时为 `None`）。
    #[serde(default)]
    pub url: Option<String>,

    /// 实际比特率（bps）。
    #[serde(default)]
    pub br: u32,

    /// 文件字节数。
    #[serde(default)]
    pub size: u64,

    /// 文件格式（如 `mp3` / `flac`），网易返回的字段名是 `type`。
    #[serde(default, rename = "type")]
    pub format: Option<String>,
}

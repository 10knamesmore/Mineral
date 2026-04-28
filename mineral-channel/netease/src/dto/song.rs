use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistDto {
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumDto {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,
}

/// 搜索结果里出现的歌曲(用 `artists`、`album`、`duration` 字段)。
#[derive(Debug, Clone, Deserialize)]
pub struct SearchSongDto {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artists: Vec<ArtistDto>,
    pub album: AlbumDto,
    #[serde(default)]
    pub duration: u64,
}

/// 专辑详情里出现的歌曲(用 `ar`、`al`、`dt` 字段)。
#[derive(Debug, Clone, Deserialize)]
pub struct AlbumSongDto {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ar: Vec<ArtistDto>,
    pub al: AlbumDto,
    #[serde(default)]
    pub dt: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SongUrlDto {
    pub id: i64,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub br: u32,
    #[serde(default)]
    pub size: u64,
    #[serde(default, rename = "type")]
    pub format: Option<String>,
}

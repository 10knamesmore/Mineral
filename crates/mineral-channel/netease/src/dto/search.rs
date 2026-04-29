use serde::Deserialize;

use super::song::{ArtistDto, SearchSongDto};

#[derive(Debug, Deserialize)]
pub struct SearchSongsResult {
    #[serde(default)]
    pub songs: Vec<SearchSongDto>,
}

#[derive(Debug, Deserialize)]
pub struct SearchAlbumsResult {
    #[serde(default)]
    pub albums: Vec<SearchAlbumDto>,
}

#[derive(Debug, Deserialize)]
pub struct SearchAlbumDto {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: Option<ArtistDto>,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "publishTime")]
    pub publish_time: i64,
    #[serde(default, rename = "picUrl")]
    pub pic_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearchPlaylistsResult {
    #[serde(default)]
    pub playlists: Vec<SearchPlaylistDto>,
}

#[derive(Debug, Deserialize)]
pub struct SearchPlaylistDto {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "coverImgUrl")]
    pub cover_img_url: Option<String>,
    #[serde(default, rename = "trackCount")]
    pub track_count: u64,
}

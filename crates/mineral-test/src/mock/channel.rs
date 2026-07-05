//! 返回固定直链的 mock [`MusicChannel`],供下载链路测试喂直链。

use async_trait::async_trait;
use mineral_channel_core::{
    ChannelCaps, Error, MusicChannel, Page, Result as ChannelResult, SearchHits,
};
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, AudioFormat, BitRate, Lyrics, MediaUrl, PlayUrl, Playlist,
    PlaylistId, Song, SongId, SourceKind,
};

/// mock channel:`song_urls` 返回指向给定 URL 的 Remote 直链(格式恒 FLAC),其余方法
/// 一律 `NotSupported`。来源恒 `NETEASE`。
pub struct UrlChannel {
    /// `song_urls` 要返回的直链(通常是 [`super::serve_once`] 的地址)。
    pub url: url::Url,
}

#[async_trait]
impl MusicChannel for UrlChannel {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .build()
    }

    async fn search_songs(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Song>> {
        Err(Error::NotSupported)
    }

    async fn search_albums(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Album>> {
        Err(Error::NotSupported)
    }

    async fn search_playlists(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Playlist>> {
        Err(Error::NotSupported)
    }

    async fn songs_detail(&self, _ids: &[SongId]) -> ChannelResult<Vec<Song>> {
        Err(Error::NotSupported)
    }

    async fn album_detail(&self, _id: &AlbumId) -> ChannelResult<Album> {
        Err(Error::NotSupported)
    }

    async fn playlist_detail(&self, _id: &PlaylistId) -> ChannelResult<Playlist> {
        Err(Error::NotSupported)
    }

    async fn song_urls(&self, ids: &[SongId], quality: BitRate) -> ChannelResult<Vec<PlayUrl>> {
        let id = ids.first().cloned().ok_or(Error::NotSupported)?;
        Ok(vec![PlayUrl {
            song_id: id,
            url: MediaUrl::Remote(self.url.clone()),
            bitrate_bps: 0,
            quality,
            size: 0,
            format: AudioFormat::Flac,
            bit_depth: Some(24),
            stream_headers: Vec::new(),
            layout: mineral_model::StreamLayout::Contiguous,
            substituted: false,
        }])
    }

    async fn lyrics(&self, _id: &SongId) -> ChannelResult<Lyrics> {
        Err(Error::NotSupported)
    }

    async fn artist_detail(&self, _id: &ArtistId) -> ChannelResult<Artist> {
        Err(Error::NotSupported)
    }

    async fn on_played(
        &self,
        _id: &SongId,
        _completed: bool,
        _listen_ms: u64,
    ) -> ChannelResult<()> {
        Ok(())
    }
}

//! 返回固定直链的 mock [`MusicChannel`],供下载链路测试喂直链。

use async_trait::async_trait;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, Error, MusicChannel, Page,
    Result as ChannelResult, SearchHits,
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
            .artist_sections(ArtistSections::new(vec![
                ArtistSectionKind::TopSongs,
                ArtistSectionKind::Albums,
            ]))
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

/// mock channel:`songs_detail` 返回预置的 [`Song`] 池(按请求 id 过滤),供需要"拉详情"的
/// 测试用(如收藏补 meta);其余方法走 trait 默认(`NotSupported` / 空)。来源可配。
pub struct DetailChannel {
    /// 该 mock 的来源(`songs_detail` 只对匹配 namespace 的 id 有意义)。
    source: SourceKind,

    /// `songs_detail` 可返回的曲目池;按请求 id 过滤后返回。
    songs: Vec<Song>,
}

impl DetailChannel {
    /// 新建。
    ///
    /// # Params:
    ///   - `source`: 该 mock 的来源
    ///   - `songs`: `songs_detail` 的曲目池(按请求 id 过滤)
    ///
    /// # Return:
    ///   mock 实例。
    pub fn new(source: SourceKind, songs: Vec<Song>) -> Self {
        Self { source, songs }
    }
}

#[async_trait]
impl MusicChannel for DetailChannel {
    fn source(&self) -> SourceKind {
        self.source
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .artist_sections(ArtistSections::new(vec![
                ArtistSectionKind::TopSongs,
                ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Song>> {
        Err(Error::NotSupported)
    }

    async fn songs_detail(&self, ids: &[SongId]) -> ChannelResult<Vec<Song>> {
        Ok(self
            .songs
            .iter()
            .filter(|s| ids.contains(&s.id))
            .cloned()
            .collect())
    }

    async fn song_urls(&self, _ids: &[SongId], _quality: BitRate) -> ChannelResult<Vec<PlayUrl>> {
        Err(Error::NotSupported)
    }
}

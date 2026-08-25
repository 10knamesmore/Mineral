//! 返回固定直链的 mock [`MusicChannel`],供下载链路测试喂直链。

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use async_trait::async_trait;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, Error, MusicChannel, Page,
    Result as ChannelResult, SearchHits,
};
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, AudioFormat, Lyrics, PlaybackMediaInfo, Playlist, PlaylistId,
    Song, SongId, SourceKind,
};
use mineral_playback::{
    DirectMedia, DirectPreparedPlayback, PlaybackProvider, PlaybackRequest, PreparedPlayback,
};
use tokio_util::sync::CancellationToken;

/// Mock channel/provider resolving a fixed remote FLAC URL.
pub struct UrlChannel {
    /// Playback provider remote URL, usually returned by [`super::serve_once`].
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

#[async_trait]
impl PlaybackProvider for UrlChannel {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    async fn resolve(
        &self,
        request: PlaybackRequest,
        _cancellation: CancellationToken,
    ) -> color_eyre::Result<Box<dyn PreparedPlayback>> {
        let info = PlaybackMediaInfo {
            song_id: request.song_id().clone(),
            bitrate_bps: None,
            quality: request.quality(),
            size: None,
            format: Some(AudioFormat::Flac),
            bit_depth: Some(24),
            substituted: false,
        };
        let media = DirectMedia::remote(
            info,
            self.url.clone(),
            Vec::new(),
            mineral_model::StreamLayout::Contiguous,
        );
        Ok(DirectPreparedPlayback::boxed(media))
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

/// mock channel:`album_detail` / `lyrics` 返回预置罐头数据(`None` → `NotSupported`),
/// `songs_detail` 按 id 过滤 `detail_songs` 池;并记录 `lyrics` 被调次数(打标去重链路
/// 据此断言「只处理一次」)。来源恒 `NETEASE`。
pub struct CannedChannel {
    /// `album_detail` 的罐头返回;`None` → `NotSupported`。
    pub album: Option<Album>,

    /// `lyrics` 的罐头返回;`None` → `NotSupported`。
    pub lyrics: Option<Lyrics>,

    /// `songs_detail` 的曲目池(按请求 id 过滤返回;空池 → 空 vec)。
    pub detail_songs: Vec<Song>,

    /// `lyrics` 被调次数。
    pub lyrics_calls: Arc<AtomicUsize>,

    /// `album_detail` 被调次数。
    pub album_calls: Arc<AtomicUsize>,
}

impl CannedChannel {
    /// 全缺省(专辑 / 歌词都 `NotSupported`,无详情池),用于「单项失败降级」场景。
    ///
    /// # Return:
    ///   无任何罐头数据的 mock。
    pub fn empty() -> Self {
        Self {
            album: None,
            lyrics: None,
            detail_songs: Vec::new(),
            lyrics_calls: Arc::new(AtomicUsize::new(0)),
            album_calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl MusicChannel for CannedChannel {
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

    async fn songs_detail(&self, ids: &[SongId]) -> ChannelResult<Vec<Song>> {
        Ok(self
            .detail_songs
            .iter()
            .filter(|s| ids.contains(&s.id))
            .cloned()
            .collect())
    }

    async fn album_detail(&self, _id: &AlbumId) -> ChannelResult<Album> {
        self.album_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.album.clone().ok_or(Error::NotSupported)
    }

    async fn playlist_detail(&self, _id: &PlaylistId) -> ChannelResult<Playlist> {
        Err(Error::NotSupported)
    }

    async fn lyrics(&self, _id: &SongId) -> ChannelResult<Lyrics> {
        self.lyrics_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.lyrics.clone().ok_or(Error::NotSupported)
    }

    async fn artist_detail(&self, _id: &ArtistId) -> ChannelResult<Artist> {
        Err(Error::NotSupported)
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
}

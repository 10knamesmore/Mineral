//! 记录收听上报并提供确定性的媒体解析，用于控制测试中的延迟、失败和直连能力。

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mineral_channel_core::{ChannelCaps, Error, MusicChannel, Page, SearchHits};
use mineral_model::{
    Album, AlbumId, Artist, Lyrics, PlaybackMediaInfo, Playlist, PlaylistId, Song, SongId,
    SourceKind,
};
use mineral_playback::{
    DirectMedia, MediaReader, OpenOptions, OpenedMedia, PlaybackProvider, PlaybackRegistry,
    PlaybackRequest, PreparedPlayback, SeekSupport,
};
use parking_lot::Mutex;

/// 记录型 mock channel:on_played 调用进 `calls`,其余方法返回 `NotSupported`。
/// `source()` 报 `NETEASE`,与 [`mineral_test::song`] 的来源对齐,确保被路由命中。
#[derive(Default)]
pub(super) struct RecordingChannel {
    /// 已记录的 on_played 调用:(歌曲 id、是否完播、收听毫秒)。
    pub(super) calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>,

    /// `liked_song_ids` 返回的远端红心集;`None` → NotSupported(favorite 导入测试用 `Some`)。
    pub(super) liked_ids: Option<rustc_hash::FxHashSet<SongId>>,

    /// `my_playlists` 返回的歌单列表;`None` → NotSupported(库聚合测试用 `Some`)。
    pub(super) playlists: Option<Vec<Playlist>>,
}

/// Deterministic playback provider used by server tests.
struct TestPlaybackProvider {
    /// Source identity served by this provider.
    source: SourceKind,

    /// Artificial resolve delay for stale-completion tests.
    delay: Duration,

    /// Whether resolve should fail instead of returning prepared media.
    fail: bool,

    /// Whether the prepared plan exposes a direct capability.
    direct: bool,
}

/// In-memory prepared media with an optional direct capability for hook projections.
struct TestPreparedPlayback {
    /// Direct capability exposed before open.
    direct: Option<DirectMedia>,

    /// Final facts returned by open even when no direct capability exists.
    info: PlaybackMediaInfo,
}

#[async_trait]
impl PlaybackProvider for TestPlaybackProvider {
    fn source(&self) -> SourceKind {
        self.source
    }

    async fn resolve(
        &self,
        request: PlaybackRequest,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> color_eyre::Result<Box<dyn PreparedPlayback>> {
        if !self.delay.is_zero() {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    return Err(color_eyre::eyre::eyre!("test playback resolve cancelled"));
                }
                () = tokio::time::sleep(self.delay) => {}
            }
        }
        if cancellation.is_cancelled() {
            return Err(color_eyre::eyre::eyre!("test playback resolve cancelled"));
        }
        if self.fail {
            return Err(color_eyre::eyre::eyre!("test playback unavailable"));
        }
        let info = PlaybackMediaInfo {
            song_id: request.song_id().clone(),
            bitrate_bps: Some(320_000),
            quality: request.quality(),
            size: Some(4),
            format: Some(mineral_model::AudioFormat::Mp3),
            bit_depth: None,
            substituted: false,
        };
        let url = url::Url::parse(&format!(
            "https://example.com/{}.mp3",
            request.song_id().value()
        ))?;
        let direct = self.direct.then(|| {
            DirectMedia::remote(
                info.clone(),
                url,
                Vec::new(),
                mineral_model::StreamLayout::Contiguous,
            )
        });
        Ok(Box::new(TestPreparedPlayback { direct, info }))
    }
}

#[async_trait]
impl PreparedPlayback for TestPreparedPlayback {
    fn direct_media(&self) -> Option<&DirectMedia> {
        self.direct.as_ref()
    }

    async fn open(self: Box<Self>, options: OpenOptions) -> color_eyre::Result<OpenedMedia> {
        if options.cancellation().is_cancelled() {
            return Err(color_eyre::eyre::eyre!("test playback open cancelled"));
        }
        let bytes = b"test".to_vec();
        let byte_len = Some(u64::try_from(bytes.len())?);
        let reader: Box<dyn MediaReader> = Box::new(Cursor::new(bytes));
        Ok(OpenedMedia::new(
            reader,
            SeekSupport::RandomAccess,
            byte_len,
            self.info,
            None,
            options.cancellation().clone(),
        ))
    }
}

/// Builds one test registry for every source represented by the channel list.
pub(super) fn test_playback_registry(
    channels: &[Arc<dyn MusicChannel>],
    delay: Duration,
    fail: bool,
    direct: bool,
) -> color_eyre::Result<PlaybackRegistry> {
    let mut sources = rustc_hash::FxHashSet::default();
    let providers = channels
        .iter()
        .filter_map(|channel| sources.insert(channel.source()).then_some(channel.source()))
        .map(|source| {
            let provider: Arc<dyn PlaybackProvider> = Arc::new(TestPlaybackProvider {
                source,
                delay,
                fail,
                direct,
            });
            provider
        })
        .collect::<Vec<Arc<dyn PlaybackProvider>>>();
    PlaybackRegistry::new(providers)
}

#[async_trait]
impl MusicChannel for RecordingChannel {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(
        &self,
        _query: &str,
        _page: Page,
    ) -> mineral_channel_core::Result<SearchHits<Song>> {
        Err(Error::NotSupported)
    }

    async fn search_albums(
        &self,
        _query: &str,
        _page: Page,
    ) -> mineral_channel_core::Result<SearchHits<Album>> {
        Err(Error::NotSupported)
    }

    async fn search_playlists(
        &self,
        _query: &str,
        _page: Page,
    ) -> mineral_channel_core::Result<SearchHits<Playlist>> {
        Err(Error::NotSupported)
    }

    async fn songs_detail(&self, _ids: &[SongId]) -> mineral_channel_core::Result<Vec<Song>> {
        Err(Error::NotSupported)
    }

    async fn album_detail(&self, _id: &AlbumId) -> mineral_channel_core::Result<Album> {
        Err(Error::NotSupported)
    }

    async fn playlist_detail(&self, _id: &PlaylistId) -> mineral_channel_core::Result<Playlist> {
        Err(Error::NotSupported)
    }

    async fn lyrics(&self, _id: &SongId) -> mineral_channel_core::Result<Lyrics> {
        Err(Error::NotSupported)
    }

    async fn artist_detail(
        &self,
        _id: &mineral_model::ArtistId,
    ) -> mineral_channel_core::Result<Artist> {
        Err(Error::NotSupported)
    }

    async fn on_played(
        &self,
        id: &SongId,
        completed: bool,
        listen_ms: u64,
    ) -> mineral_channel_core::Result<()> {
        self.calls.lock().push((id.clone(), completed, listen_ms));
        Ok(())
    }

    async fn liked_song_ids(&self) -> mineral_channel_core::Result<rustc_hash::FxHashSet<SongId>> {
        self.liked_ids.clone().ok_or(Error::NotSupported)
    }

    async fn my_playlists(&self) -> mineral_channel_core::Result<Vec<Playlist>> {
        self.playlists.clone().ok_or(Error::NotSupported)
    }
}

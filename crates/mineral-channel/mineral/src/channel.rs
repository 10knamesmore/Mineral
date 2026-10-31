//! Mineral 内建来源的歌单适配器。

use std::sync::Arc;

use async_trait::async_trait;
use mineral_channel_core::collection::fresh_collection;
use mineral_channel_core::fetch::fresh_snapshot;
use mineral_channel_core::store::LibraryStore;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, CollectionStream, Error, FetchStream,
    MusicChannel, Page, PageResult, PlaylistDetail, PlaylistLoad, Result,
};
use mineral_model::CachePolicy;
use mineral_model::{Playlist, PlaylistEntry, PlaylistId, Song, SongId, SourceKind};

/// `mineral:favorites` 聚合收藏歌单的 id。
///
/// # Return:
///   聚合收藏歌单的 [`PlaylistId`]。
pub fn favorites_playlist_id() -> PlaylistId {
    PlaylistId::new(SourceKind::MINERAL, "favorites")
}

/// Mineral 歌单适配器，读取跨来源 Favorites 与用户保存的歌单。
///
/// 搜索 / 详情一律 `NotSupported`——歌单里每首歌的 id 保留**原源** namespace,
/// 播放按 id 路由到对应 Playback provider,本 channel 不参与播放资源解析。
pub struct MineralChannel {
    /// daemon 注入的跨来源库访问。
    library: Arc<dyn LibraryStore>,
}

impl MineralChannel {
    /// 新建聚合 channel。
    ///
    /// # Params:
    ///   - `library`: 跨来源库访问接口
    ///
    /// # Return:
    ///   聚合 channel 实例。
    pub fn new(library: Arc<dyn LibraryStore>) -> Self {
        Self { library }
    }

    /// 从跨来源查询重建聚合收藏歌单(name `Favorites`,曲目按收藏时间降序)。
    ///
    /// 计数与详情都只包含有歌曲资料的收藏；仅请求计数时不加载整份歌曲列表。
    ///
    /// # Params:
    ///   - `with_songs`: `false` 只出计数(歌单列表用,省 IPC 载荷 + 省 DB 重建),`true` 带全曲目
    ///
    /// # Return:
    ///   聚合收藏歌单。
    async fn build_favorites(&self, with_songs: bool) -> Result<Playlist> {
        let (track_count, songs) = if with_songs {
            let songs = self
                .library
                .loved_songs()
                .await
                .map_err(|source| Error::Storage {
                    source: Box::new(source),
                })?;
            let count = u64::try_from(songs.len()).map_err(|source| Error::Parse {
                source: Box::new(source),
            })?;
            (count, songs)
        } else {
            let count = self
                .library
                .loved_count()
                .await
                .map_err(|source| Error::Storage {
                    source: Box::new(source),
                })?;
            (count, Vec::new())
        };
        Ok(Playlist::builder()
            .id(favorites_playlist_id())
            .name("Favorites".to_owned())
            .track_count(track_count)
            .entries(PlaylistEntry::enumerate(songs))
            .build())
    }
}

#[async_trait]
impl MusicChannel for MineralChannel {
    fn source(&self) -> SourceKind {
        SourceKind::MINERAL
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            // 聚合源:artist 详情沿用音乐源形态(热门曲 + 专辑)。
            .artist_sections(ArtistSections::new(vec![
                ArtistSectionKind::TopSongs,
                ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(&self, _query: &str, _page: Page) -> Result<PageResult<Song>> {
        Err(Error::NotSupported)
    }

    async fn songs_detail(&self, _ids: &[SongId], _cache: CachePolicy) -> Result<Vec<Song>> {
        Err(Error::NotSupported)
    }

    fn my_playlists(&self, _cache: CachePolicy) -> CollectionStream<'_, Playlist> {
        fresh_collection(async {
            let mut playlists = vec![self.build_favorites(/*with_songs*/ false).await?];
            playlists.extend(self.library.user_playlists().await.map_err(|source| {
                Error::Storage {
                    source: Box::new(source),
                }
            })?);
            Ok(playlists)
        })
    }

    fn playlist_detail<'a>(
        &'a self,
        id: &'a PlaylistId,
        _load: PlaylistLoad,
        _cache: CachePolicy,
    ) -> FetchStream<'a, PlaylistDetail> {
        fresh_snapshot(async move {
            if id.namespace() != SourceKind::MINERAL {
                return Err(Error::NotSupported);
            }
            if *id != favorites_playlist_id() {
                return self
                    .library
                    .user_playlist(id)
                    .await
                    .map_err(|source| Error::Storage {
                        source: Box::new(source),
                    })?
                    .map(PlaylistDetail::complete)
                    .ok_or(Error::NotFound);
            }
            self.build_favorites(/*with_songs*/ true)
                .await
                .map(PlaylistDetail::complete)
        })
    }
}

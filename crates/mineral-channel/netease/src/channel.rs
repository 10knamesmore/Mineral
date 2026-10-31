//! `impl MusicChannel for NeteaseChannel`。
//!
//! 业务层:组合 `api/` 端点(纯协议 → 类型化 DTO)与 `convert`(DTO → mineral-model 映射),
//! 收敛错误为 `mineral_channel_core::Error`,让上层面向 trait 编程。端点调用与 model 映射各
//! 在其层,本文件只做编排与业务决策(如详情聚合多端点、歌单缓存)。

use async_trait::async_trait;
use futures_util::StreamExt;
use isahc::cookies::{Cookie, CookieJar};
use mineral_channel_core::collection::{albums_with_cache, playlists_with_cache};
use mineral_channel_core::store::NamespaceStore;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, CollectionStream, Error, FetchStream,
    MusicChannel, Page, PageResult, PlaylistDetail, PlaylistLoad, Result,
};
use mineral_model::CachePolicy;
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, Lyrics, Playlist, PlaylistId, SearchKind, Song, SongId,
    SourceKind, UserId,
};
use mineral_playback::{
    DirectPreparedPlayback, Error as PlaybackError, PlaybackProvider, PlaybackRequest,
    PreparedPlayback,
};
use rustc_hash::FxHashSet;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::error::{ApiCodeError, Error as NeteaseError, Result as NeteaseResult};

use crate::album::AlbumLoader;
use crate::api;
use crate::config::NeteaseConfig;
use crate::convert;
use crate::request::RequestPolicies;
use crate::transport::Transport;

/// 绑定登录凭证与用户身份的网易云 channel。
pub struct NeteaseChannel {
    /// 网易云请求的 HTTP 通道(带 cookie jar、加密、UA 处理)。
    transport: Transport,

    /// 当前实例绑定的登录用户 uid。
    user_id: UserId,

    /// 本地持久化句柄;未提供时跳过缓存读写。
    persist: Option<Arc<dyn NamespaceStore>>,

    /// 歌单曲目请求批次，在 channel 构造时由来源配置注入。
    playlist: crate::playlist::PlaylistLoader,

    /// 随实例共享的端点发送额度与限流退避策略。
    requests: RequestPolicies,

    /// 专辑详情的持久缓存读取与到期刷新。
    album: AlbumLoader,
}

impl NeteaseChannel {
    /// 同时注入 `MUSIC_U` 与登录用户 uid,得到一个有「我的歌单」上下文的 channel。
    ///
    /// # Params:
    ///   - `config`: HTTP 客户端配置
    ///   - `music_u`: 网易云核心登录 cookie 值
    ///   - `user_id`: 登录用户 uid(`my_playlists` 内部转发给 `user_playlists`)
    ///   - `persist`: 持久化句柄;传 `None` 可跳过本地落盘
    pub fn with_credential(
        config: &NeteaseConfig,
        music_u: &str,
        user_id: UserId,
        persist: Option<Arc<dyn NamespaceStore>>,
    ) -> crate::Result<Self> {
        let jar = CookieJar::new();
        let url = "https://music.163.com"
            .parse()
            .map_err(|source| NeteaseError::Network {
                operation: "parse Netease base URI",
                source: Box::new(source),
            })?;
        let cookie = Cookie::builder("MUSIC_U", music_u)
            .domain("music.163.com")
            .path("/")
            .build()
            .map_err(|source| NeteaseError::Network {
                operation: "build MUSIC_U cookie",
                source: Box::new(source),
            })?;
        jar.set(cookie, &url)
            .map_err(|source| NeteaseError::Network {
                operation: "set MUSIC_U cookie",
                source: Box::new(source),
            })?;
        Ok(Self {
            transport: Transport::from_cookie_jar(config, jar)?,
            user_id,
            persist,
            playlist: crate::playlist::PlaylistLoader::new(config.playlist_fetch()),
            requests: RequestPolicies::new(config.requests()),
            album: AlbumLoader::new(config.album_cache()),
        })
    }
}

/// 将网易云领域错误映射为跨 channel 分类，保留原始 cause。
fn map_err(error: NeteaseError) -> Error {
    match error {
        NeteaseError::Api(ApiCodeError { code: 301, .. }) => Error::AuthRequired,
        error if error.is_rate_limited() => Error::RateLimited,
        NeteaseError::Api(ApiCodeError { code, message }) => Error::Api { code, message },
        NeteaseError::Network { .. } => Error::Network {
            source: Box::new(error),
        },
        NeteaseError::Parse { .. } | NeteaseError::Serialize(_) | NeteaseError::WebUrl(_) => {
            Error::Parse {
                source: Box::new(error),
            }
        }
        NeteaseError::InvalidData { field } => Error::InvalidData { field },
        NeteaseError::Storage(_)
        | NeteaseError::Path(_)
        | NeteaseError::File { .. }
        | NeteaseError::InvalidCredentialPath { .. } => Error::Storage {
            source: Box::new(error),
        },
        NeteaseError::PlaylistOffset(_) => Error::Parse {
            source: Box::new(error),
        },
        NeteaseError::LoginAccount { .. }
        | NeteaseError::Qr(_)
        | NeteaseError::LoginCancelled
        | NeteaseError::LoginExpired
        | NeteaseError::LoginStatus { .. } => Error::Parse {
            source: Box::new(error),
        },
    }
}

#[async_trait]
impl MusicChannel for NeteaseChannel {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(vec![
                SearchKind::Song,
                SearchKind::Artist,
                SearchKind::Album,
                SearchKind::Playlist,
            ])
            .playlist_edit(true)
            // 音乐源:artist 详情有热门曲区 + 专辑区。
            .artist_sections(ArtistSections::new(vec![
                ArtistSectionKind::TopSongs,
                ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(&self, query: &str, page: Page) -> Result<PageResult<Song>> {
        let dto = api::search::search_songs(&self.transport, query, page.offset, page.limit)
            .await
            .map_err(map_err)?;
        // 响应不带总数/总页数元信息,has_more 留 None(上层按「短页即榨干」推断)。
        Ok(dto
            .songs
            .into_iter()
            .map(convert::album_song_to_model)
            .collect::<NeteaseResult<Vec<Song>>>()
            .map_err(map_err)?
            .into())
    }

    async fn search_albums(&self, query: &str, page: Page) -> Result<PageResult<Album>> {
        let dto = api::search::search_albums(&self.transport, query, page.offset, page.limit)
            .await
            .map_err(map_err)?;
        // 搜索只有元信息,曲目按需走 album_detail(传空 songs)。
        Ok(dto
            .albums
            .into_iter()
            .map(|a| convert::album_dto_to_model(a, Vec::new()))
            .collect::<NeteaseResult<Vec<Album>>>()
            .map_err(map_err)?
            .into())
    }

    async fn search_playlists(&self, query: &str, page: Page) -> Result<PageResult<Playlist>> {
        let dto = api::search::search_playlists(&self.transport, query, page.offset, page.limit)
            .await
            .map_err(map_err)?;
        Ok(dto
            .playlists
            .into_iter()
            .map(convert::search_playlist_to_model)
            .collect::<NeteaseResult<Vec<Playlist>>>()
            .map_err(map_err)?
            .into())
    }

    async fn search_artists(&self, query: &str, page: Page) -> Result<PageResult<Artist>> {
        let dto = api::search::search_artists(&self.transport, query, page.offset, page.limit)
            .await
            .map_err(map_err)?;
        Ok(dto
            .artists
            .into_iter()
            .map(convert::search_artist_to_model)
            .collect::<NeteaseResult<Vec<Artist>>>()
            .map_err(map_err)?
            .into())
    }

    /// artist 详情:并发取「详情(简介/计数/热门曲)」与「粉丝数」两端点,聚合成完整 [`Artist`]。
    ///
    /// `/weapi/v1/artist/{id}` 顶层不带粉丝数,粉丝数只有 `/api/artist/follow/count/get` 给;两端点
    /// 并发打、就地聚合。详情端点失败则整体失败(主数据);粉丝数端点失败降级 0(非致命,warn 留痕)。
    async fn artist_detail(&self, id: &ArtistId) -> Result<Artist> {
        let (detail, fans) = tokio::join!(
            api::artist::detail(&self.transport, id),
            api::artist::follow_count(&self.transport, id),
        );
        let detail = detail.map_err(map_err)?;
        let fans = fans.map(Some).unwrap_or_else(|e| {
            mineral_log::warn!(
                target: "netease",
                artist = id.value(),
                error = mineral_log::chain(&e),
                "artist follow count fetch failed; fans unknown"
            );
            None
        });
        convert::artist_detail_to_model(detail, fans).map_err(map_err)
    }

    async fn artist_albums(&self, id: &ArtistId, page: Page) -> Result<PageResult<Album>> {
        let dto = api::artist::albums(&self.transport, id, page.offset, page.limit)
            .await
            .map_err(map_err)?;
        convert::artist_albums_to_model(dto).map_err(map_err)
    }

    async fn create_playlist(&self, name: &str) -> Result<Playlist> {
        let dto = api::playlist_edit::create_playlist(&self.transport, name)
            .await
            .map_err(map_err)?;
        // 建单响应只带新歌单元信息,无曲目。
        convert::playlist_info_to_model(&dto.playlist, Vec::new()).map_err(map_err)
    }

    async fn delete_playlist(&self, id: &PlaylistId) -> Result<()> {
        api::playlist_edit::delete_playlist(&self.transport, id)
            .await
            .map_err(map_err)
    }

    async fn playlist_add_songs(&self, id: &PlaylistId, songs: &[SongId]) -> Result<()> {
        api::playlist_edit::playlist_add_songs(&self.transport, id, songs)
            .await
            .map_err(map_err)
    }

    async fn playlist_remove_songs(&self, id: &PlaylistId, songs: &[SongId]) -> Result<()> {
        api::playlist_edit::playlist_remove_songs(&self.transport, id, songs)
            .await
            .map_err(map_err)
    }

    async fn rename_playlist(&self, id: &PlaylistId, name: &str) -> Result<()> {
        api::playlist_edit::rename_playlist(&self.transport, id, name)
            .await
            .map_err(map_err)
    }

    async fn set_playlist_description(&self, id: &PlaylistId, desc: &str) -> Result<()> {
        api::playlist_edit::set_playlist_description(&self.transport, id, desc)
            .await
            .map_err(map_err)
    }

    async fn songs_detail(&self, ids: &[SongId], _cache: CachePolicy) -> Result<Vec<Song>> {
        let dtos = api::song::songs_detail(&self.transport, ids)
            .await
            .map_err(map_err)?;
        dtos.into_iter()
            .map(convert::album_song_to_model)
            .collect::<NeteaseResult<Vec<_>>>()
            .map_err(map_err)
    }

    fn album_detail<'a>(&'a self, id: &'a AlbumId, cache: CachePolicy) -> FetchStream<'a, Album> {
        self.album
            .load(
                &self.transport,
                &self.requests.album_detail,
                self.persist.as_deref(),
                id,
                cache,
            )
            .map(|result| result.map_err(map_err))
            .boxed()
    }

    /// 预览或完整加载由歌单模块执行，远端请求失败映射为公共 channel 错误。
    fn playlist_detail<'a>(
        &'a self,
        id: &'a PlaylistId,
        load: PlaylistLoad,
        cache: CachePolicy,
    ) -> FetchStream<'a, PlaylistDetail> {
        self.playlist
            .load(&self.transport, self.persist.as_deref(), id, load, cache)
            .map(|result| result.map_err(map_err))
            .boxed()
    }

    async fn lyrics(&self, id: &SongId) -> Result<Lyrics> {
        api::lyric::lyrics(&self.transport, &self.requests.lyrics, id)
            .await
            .map_err(map_err)
    }

    async fn user_playlists(&self, uid: &UserId) -> Result<Vec<Playlist>> {
        let dto = api::playlist::user_playlists(&self.transport, uid)
            .await
            .map_err(map_err)?;
        // 列表项只有元信息,无曲目(曲目按需走 playlist_detail)。
        dto.playlist
            .iter()
            .map(|info| convert::playlist_info_to_model(info, Vec::new()))
            .collect::<NeteaseResult<Vec<_>>>()
            .map_err(map_err)
    }

    fn my_playlists(&self, cache: CachePolicy) -> CollectionStream<'_, Playlist> {
        playlists_with_cache(
            self.persist.as_deref(),
            cache,
            self.user_playlists(&self.user_id),
        )
    }

    fn my_albums(&self, cache: CachePolicy) -> CollectionStream<'_, Album> {
        albums_with_cache(self.persist.as_deref(), cache, async {
            const PAGE_SIZE: usize = 50;
            let mut albums = Vec::new();
            let mut offset = 0;
            loop {
                let page = api::album::saved(&self.transport, offset, PAGE_SIZE)
                    .await
                    .map_err(map_err)?;
                let count = page.data.len();
                mineral_log::debug!(target: "netease", offset, count, more = page.has_more,
                "saved album page fetched");
                albums.extend(
                    page.data
                        .into_iter()
                        .map(convert::saved_album_to_model)
                        .collect::<NeteaseResult<Vec<_>>>()
                        .map_err(map_err)?,
                );
                if !page.has_more {
                    break;
                }
                offset += PAGE_SIZE;
            }
            mineral_log::info!(target: "netease", albums = albums.len(), "saved albums fetched");
            Ok(albums)
        })
    }

    /// 拉网易云账号远端红心的歌曲 ID 集合(纯远端)。
    ///
    /// 供 server 导入本地 persist(add-only)用，本地收藏的读取与合并由 server 处理。
    async fn liked_song_ids(&self) -> Result<FxHashSet<SongId>> {
        api::user::liked_song_ids(&self.transport, &self.user_id)
            .await
            .map_err(map_err)
    }

    /// 把一首歌的红心状态镜像到网易云远端(纯远端镜像)。
    ///
    /// 本地 persist 由 server 统一写，这里只同步远端。
    async fn set_loved(&self, id: &SongId, loved: bool) -> Result<()> {
        api::song::like_song(&self.transport, id, loved)
            .await
            .map_err(map_err)
    }

    /// 从回忆坐标查询当前账号的远端累计播放次数。
    async fn remote_play_count(&self, id: &SongId) -> Result<u32> {
        api::song::remote_play_count(&self.transport, id)
            .await
            .map_err(map_err)
    }
}

#[async_trait]
impl PlaybackProvider for NeteaseChannel {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    /// Resolves a Netease song using the v1 endpoint with legacy fallback.
    async fn resolve(
        &self,
        request: PlaybackRequest,
        cancellation: CancellationToken,
    ) -> mineral_playback::Result<Box<dyn PreparedPlayback>> {
        if cancellation.is_cancelled() {
            return Err(PlaybackError::Cancelled);
        }
        let ids = [request.song_id().clone()];
        if let Ok(dtos) = api::song::song_url_v1(&self.transport, &ids, request.quality()).await {
            if convert::all_explicitly_unavailable(&dtos) {
                return Err(PlaybackError::Provider {
                    source: Box::new(NeteaseError::InvalidData {
                        field: "playable Netease media",
                    }),
                });
            }
            if let Some(media) = convert::to_direct_media(dtos).into_iter().next() {
                return Ok(DirectPreparedPlayback::boxed(media));
            }
        }
        if cancellation.is_cancelled() {
            return Err(PlaybackError::Cancelled);
        }
        let dtos = api::song::song_url_legacy(&self.transport, &ids, request.quality())
            .await
            .map_err(|source| PlaybackError::Provider {
                source: Box::new(source),
            })?;
        let media = convert::to_direct_media(dtos)
            .into_iter()
            .next()
            .ok_or_else(|| PlaybackError::Provider {
                source: Box::new(NeteaseError::InvalidData {
                    field: "playable Netease media",
                }),
            })?;
        Ok(DirectPreparedPlayback::boxed(media))
    }
}

#[cfg(test)]
mod tests {
    use crate::error::ApiCodeError;
    use mineral_channel_core::Error;

    /// 按 code 映射 API 错误，其余分类保留底层原因。
    #[test]
    fn map_err_translates_api_codes() {
        let f = |code: i64| {
            super::map_err(crate::Error::Api(ApiCodeError {
                code,
                message: Some(String::from("m")),
            }))
        };
        assert!(matches!(f(301), Error::AuthRequired));
        assert!(matches!(f(512), Error::RateLimited));
        assert!(matches!(f(405), Error::RateLimited));
        assert!(matches!(f(502), Error::Api { code: 502, .. }));
        let network = super::map_err(crate::Error::Network {
            operation: "send request",
            source: Box::new(std::io::Error::other("timeout")),
        });
        assert!(matches!(&network, Error::Network { .. }));
        assert!(std::error::Error::source(&network).is_some());
    }

    /// 解析失败保留字段路径和原始 serde 错误。
    #[test]
    fn map_err_preserves_parse_source() -> color_eyre::Result<()> {
        let parse = crate::wire::de::from_value::<Vec<i32>>(serde_json::json!(["invalid"]))
            .err()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected invalid element"))?;
        let error = super::map_err(parse);
        assert!(matches!(&error, Error::Parse { .. }));
        assert!(std::error::Error::source(&error).is_some());
        Ok(())
    }
}

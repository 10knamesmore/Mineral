//! `impl MusicChannel for NeteaseChannel`。
//!
//! 业务层:组合 `api/` 端点(纯协议 → 类型化 DTO)与 `convert`(DTO → mineral-model 映射),
//! 收敛错误为 `mineral_channel_core::Error`,让上层面向 trait 编程。端点调用与 model 映射各
//! 在其层,本文件只做编排与业务决策(如详情聚合多端点、歌单缓存)。

use async_trait::async_trait;
use isahc::cookies::{Cookie, CookieJar};
use mineral_channel_core::store::NamespaceStore;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, Error, MusicChannel, Page, PageResult, Result,
};
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

use crate::error::{ApiCodeError, Error as NeteaseError};

use crate::album::AlbumLoader;
use crate::api;
use crate::config::NeteaseConfig;
use crate::convert;
use crate::request::RequestPolicies;
use crate::transport::Transport;

/// 网易云 channel 实例。
pub struct NeteaseChannel {
    /// 网易云请求的 HTTP 通道(带 cookie jar、加密、UA 处理)。
    transport: Transport,

    /// 当前实例绑定的登录用户 uid;`None` 时 `my_playlists` 返回 `NotSupported`。
    user_id: Option<UserId>,

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
    /// 构造一个未登录的 channel(只能跑公开端点)。需要登录态请走 [`Self::with_cookie`] / [`Self::with_credential`]。
    ///
    /// # Params:
    ///   - `config`: HTTP 客户端配置
    ///   - `persist`: 持久化句柄;传 `None` 可跳过本地落盘
    pub fn new(
        config: &NeteaseConfig,
        persist: Option<Arc<dyn NamespaceStore>>,
    ) -> crate::Result<Self> {
        Ok(Self {
            transport: Transport::new(config)?,
            user_id: None,
            persist,
            playlist: crate::playlist::PlaylistLoader::new(config.playlist_fetch()),
            requests: RequestPolicies::new(config.requests()),
            album: AlbumLoader::new(config.album_cache()),
        })
    }

    /// 仅用 `MUSIC_U` cookie 构造 channel,不绑 uid。
    ///
    /// `music_u` 通常从浏览器 `Application → Cookies → music.163.com` 复制。
    /// 这种 channel 能跑 search / 详情类端点,但 [`MusicChannel::my_playlists`]
    /// 因为不知道 uid 会返回 [`mineral_channel_core::Error::NotSupported`];
    /// 同时绑 uid 的入口走 [`Self::with_credential`]。
    ///
    /// # Params:
    ///   - `config`: HTTP 客户端配置
    ///   - `music_u`: 网易云核心登录 cookie 值
    ///   - `persist`: 持久化句柄;传 `None` 可跳过本地落盘
    pub fn with_cookie(
        config: &NeteaseConfig,
        music_u: &str,
        persist: Option<Arc<dyn NamespaceStore>>,
    ) -> crate::Result<Self> {
        Self::build(config, music_u, None, persist)
    }

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
        Self::build(config, music_u, Some(user_id), persist)
    }

    /// `with_cookie` / `with_credential` 的共享实现:把 `MUSIC_U` 塞进 jar,再套一层 [`Transport`]。
    ///
    /// # Params:
    ///   - `config`: HTTP 客户端配置
    ///   - `music_u`: 网易云核心登录 cookie 值
    ///   - `user_id`: 可选的登录 uid
    ///   - `persist`: 持久化句柄
    fn build(
        config: &NeteaseConfig,
        music_u: &str,
        user_id: Option<UserId>,
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

    /// 暴露内部 transport,给一些不在 `MusicChannel` 范围内的端点用
    /// (例如二维码登录 GetKey/CheckQR、ping 等)。
    pub fn transport(&self) -> &Transport {
        &self.transport
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
        NeteaseError::Parse { .. } | NeteaseError::Serialize(_) => Error::Parse {
            source: Box::new(error),
        },
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
            .song_web_url(Some("https://music.163.com/song?id={id}".to_owned()))
            .playlist_web_url(Some("https://music.163.com/playlist?id={id}".to_owned()))
            .album_web_url(Some("https://music.163.com/album?id={id}".to_owned()))
            .artist_web_url(Some("https://music.163.com/artist?id={id}".to_owned()))
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
            .collect::<Vec<Song>>()
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
            .collect::<Vec<Album>>()
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
            .collect::<Vec<Playlist>>()
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
            .collect::<Vec<Artist>>()
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
        Ok(convert::artist_detail_to_model(detail, fans))
    }

    async fn artist_albums(&self, id: &ArtistId, page: Page) -> Result<PageResult<Album>> {
        let dto = api::artist::albums(&self.transport, id, page.offset, page.limit)
            .await
            .map_err(map_err)?;
        Ok(convert::artist_albums_to_model(dto))
    }

    async fn create_playlist(&self, name: &str) -> Result<Playlist> {
        let dto = api::playlist_edit::create_playlist(&self.transport, name)
            .await
            .map_err(map_err)?;
        // 建单响应只带新歌单元信息,无曲目。
        Ok(convert::playlist_info_to_model(&dto.playlist, Vec::new()))
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

    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>> {
        let dtos = api::song::songs_detail(&self.transport, ids)
            .await
            .map_err(map_err)?;
        Ok(dtos.into_iter().map(convert::album_song_to_model).collect())
    }

    async fn album_detail(&self, id: &AlbumId) -> Result<Album> {
        self.album
            .load(
                &self.transport,
                &self.requests.album_detail,
                self.persist.as_deref(),
                id,
            )
            .await
            .map_err(map_err)
    }

    /// 预览或完整加载由歌单模块执行，远端请求失败映射为公共 channel 错误。
    async fn playlist_detail(
        &self,
        id: &PlaylistId,
        load: mineral_channel_core::PlaylistLoad,
    ) -> Result<mineral_channel_core::PlaylistDetail> {
        self.playlist
            .load(&self.transport, self.persist.as_deref(), id, load)
            .await
            .map_err(map_err)
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
        Ok(dto
            .playlist
            .iter()
            .map(|info| convert::playlist_info_to_model(info, Vec::new()))
            .collect())
    }

    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        match self.user_id.as_ref() {
            Some(uid) => self.user_playlists(uid).await,
            None => Err(Error::NotSupported),
        }
    }

    async fn my_albums(&self) -> Result<Vec<Album>> {
        if self.user_id.is_none() {
            return Err(Error::NotSupported);
        }
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
            albums.extend(page.data.into_iter().map(convert::saved_album_to_model));
            if !page.has_more {
                break;
            }
            offset += PAGE_SIZE;
        }
        mineral_log::info!(target: "netease", albums = albums.len(), "saved albums fetched");
        Ok(albums)
    }

    /// 拉网易云账号远端红心的歌曲 ID 集合(纯远端)。
    ///
    /// 供 server 导入本地 persist(add-only)用。未登录 → [`Error::NotSupported`]:
    /// 本地 favorited 集由 server 从 persist 读,不在此降级(本地为准的合并在 server 一侧)。
    ///
    /// # Return:
    ///   远端红心 id 集;未登录 / 远端失败为 `Err`。
    async fn liked_song_ids(&self) -> Result<FxHashSet<SongId>> {
        let Some(uid) = self.user_id.as_ref() else {
            return Err(Error::NotSupported);
        };
        api::user::liked_song_ids(&self.transport, uid)
            .await
            .map_err(map_err)
    }

    /// 把一首歌的红心状态镜像到网易云远端(纯远端镜像)。
    ///
    /// 本地 persist 由 server 统一写(事实来源),这里只同步远端;未登录无远端可打 →
    /// [`Error::NotSupported`](server 视为该源无远端镜像,不影响本地已写)。
    async fn set_loved(&self, id: &SongId, loved: bool) -> Result<()> {
        if self.user_id.is_none() {
            return Err(Error::NotSupported);
        }
        api::song::like_song(&self.transport, id, loved)
            .await
            .map_err(map_err)
    }

    /// 远端真实累计播放次数:登录(有 uid)才查回忆坐标;未登录返回 [`Error::NotSupported`]。
    async fn remote_play_count(&self, id: &SongId) -> Result<u32> {
        if self.user_id.is_none() {
            return Err(Error::NotSupported);
        }
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
    use std::time::Duration;

    use mineral_channel_core::{Error, MusicChannel};
    use mineral_model::{SongId, SourceKind};

    use crate::NeteaseChannel;
    use crate::config::{AlbumCacheConfig, NeteaseConfig, RequestsConfig};
    use crate::error::ApiCodeError;

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

    /// favorite 方法收窄为**纯远端**:匿名 channel(未登录)无远端可查/可打,
    /// `liked_song_ids` 与 `set_loved` 都返回 [`Error::NotSupported`]。
    ///
    /// 本地 favorited 集与本地写入统一由 server 经 persist 负责(不在 channel 降级),
    /// 故匿名 channel 不读本地 loved_ids。
    #[tokio::test]
    async fn favorite_methods_not_supported_when_anonymous() -> color_eyre::Result<()> {
        let defaults = mineral_server::config::DaemonConfig::defaults()?;
        let source = defaults.sources().netease();
        let requests = source.requests();
        let config = NeteaseConfig::builder()
            .album_cache(
                AlbumCacheConfig::builder()
                    .ttl_days(*source.album_cache().ttl_days())
                    .ttl_jitter_days(*source.album_cache().ttl_jitter_days())
                    .build(),
            )
            .playlist_fetch(
                crate::config::PlaylistFetchConfig::builder()
                    .batch_size(std::num::NonZeroUsize::new(500).unwrap())
                    .max_concurrent(std::num::NonZeroUsize::new(3).unwrap())
                    .build(),
            )
            .requests(
                RequestsConfig::builder()
                    .album_detail_requests_per_second(*requests.album_detail_requests_per_second())
                    .retry_delays(
                        requests
                            .retry_delays_ms()
                            .iter()
                            .map(|delay| Duration::from_millis(delay.get()))
                            .collect(),
                    )
                    .build(),
            )
            .max_connections(0)
            .proxy(None)
            .timeout_secs(100)
            .build();
        let channel = NeteaseChannel::new(&config, None)?;

        assert!(
            matches!(channel.liked_song_ids().await, Err(Error::NotSupported)),
            "匿名 liked_song_ids 应 NotSupported(纯远端,不降级本地)"
        );
        assert!(matches!(
            channel.my_albums().await,
            Err(Error::NotSupported)
        ));
        let id = SongId::new(SourceKind::NETEASE, "10001");
        assert!(
            matches!(
                channel.set_loved(&id, /*loved*/ true).await,
                Err(Error::NotSupported)
            ),
            "匿名 set_loved 应 NotSupported(纯远端镜像,不写本地)"
        );
        Ok(())
    }
}

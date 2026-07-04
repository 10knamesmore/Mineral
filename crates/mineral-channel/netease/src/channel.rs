//! `impl MusicChannel for NeteaseChannel`。
//!
//! 业务层:组合 `api/` 端点(纯协议 → 类型化 DTO)与 `convert`(DTO → mineral-model 映射),
//! 收敛错误为 `mineral_channel_core::Error`,让上层面向 trait 编程。端点调用与 model 映射各
//! 在其层,本文件只做编排与业务决策(如详情聚合多端点、歌单缓存)。

use async_trait::async_trait;
use color_eyre::eyre::eyre;
use isahc::cookies::{Cookie, CookieJar};
use mineral_channel_core::{
    ChannelCaps, Credential, Error, MusicChannel, Page, Result, SearchHits,
};
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, SearchKind,
    Song, SongId, SourceKind, UserId,
};
use mineral_persist::ServerStore;
use rustc_hash::FxHashSet;

use crate::error::ApiCodeError;

use crate::api;
use crate::config::NeteaseConfig;
use crate::convert;
use crate::playlist_cache;
use crate::transport::Transport;

/// 网易云 channel 实例。
pub struct NeteaseChannel {
    /// 网易云请求的 HTTP 通道(带 cookie jar、加密、UA 处理)。
    transport: Transport,

    /// 当前实例绑定的登录用户 uid;`None` 时 `my_playlists` 返回 `NotSupported`。
    user_id: Option<UserId>,

    /// 本地持久化句柄;降级(`ServerStore::disabled()`)时所有读写 no-op,播放不受影响。
    persist: ServerStore,
}

impl NeteaseChannel {
    /// 构造一个未登录的 channel(只能跑公开端点)。需要登录态请走 [`Self::with_cookie`] / [`Self::with_credential`]。
    ///
    /// # Params:
    ///   - `config`: HTTP 客户端配置
    ///   - `persist`: 持久化句柄;传 [`ServerStore::disabled()`] 可跳过本地落盘
    pub fn new(config: &NeteaseConfig, persist: ServerStore) -> color_eyre::Result<Self> {
        Ok(Self {
            transport: Transport::new(config)?,
            user_id: None,
            persist,
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
    ///   - `persist`: 持久化句柄;传 [`ServerStore::disabled()`] 可跳过本地落盘
    pub fn with_cookie(
        config: &NeteaseConfig,
        music_u: &str,
        persist: ServerStore,
    ) -> color_eyre::Result<Self> {
        Self::build(config, music_u, None, persist)
    }

    /// 同时注入 `MUSIC_U` 与登录用户 uid,得到一个有「我的歌单」上下文的 channel。
    ///
    /// # Params:
    ///   - `config`: HTTP 客户端配置
    ///   - `music_u`: 网易云核心登录 cookie 值
    ///   - `user_id`: 登录用户 uid(`my_playlists` 内部转发给 `user_playlists`)
    ///   - `persist`: 持久化句柄;传 [`ServerStore::disabled()`] 可跳过本地落盘
    pub fn with_credential(
        config: &NeteaseConfig,
        music_u: &str,
        user_id: UserId,
        persist: ServerStore,
    ) -> color_eyre::Result<Self> {
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
        persist: ServerStore,
    ) -> color_eyre::Result<Self> {
        let jar = CookieJar::new();
        let url = "https://music.163.com"
            .parse()
            .map_err(|e| eyre!("parse netease base uri: {e}"))?;
        let cookie = Cookie::builder("MUSIC_U", music_u)
            .domain("music.163.com")
            .path("/")
            .build()
            .map_err(|e| eyre!("build cookie: {e}"))?;
        jar.set(cookie, &url)
            .map_err(|e| eyre!("set cookie: {e}"))?;
        Ok(Self {
            transport: Transport::from_cookie_jar(config, jar)?,
            user_id,
            persist,
        })
    }

    /// 暴露内部 transport,给一些不在 `MusicChannel` 范围内的端点用
    /// (例如二维码登录 GetKey/CheckQR、ping 等)。
    pub fn transport(&self) -> &Transport {
        &self.transport
    }
}

/// 把 api 层的 `color_eyre::Report` 收敛到 channel-core 错误。
///
/// 携带 [`ApiCodeError`] 的按 code 结构化映射:301 → `AuthRequired`、
/// 512(风控/歌单容量,远端不区分)→ `RateLimited`、其余透传 `Api`
/// (含加歌重复的 502,由 TUI 翻译成"已在歌单中");纯网络/解析类
/// Report 落 `Error::Other` 兜底。
fn map_err(e: color_eyre::Report) -> Error {
    match e.downcast_ref::<ApiCodeError>() {
        Some(api) => match api.code {
            301 => Error::AuthRequired,
            512 => Error::RateLimited,
            _ => Error::Api {
                code: api.code,
                message: api.message.clone(),
            },
        },
        None => Error::Other(e),
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
            .song_web_url(Some("https://music.163.com/song?id={id}".to_owned()))
            .playlist_web_url(Some("https://music.163.com/playlist?id={id}".to_owned()))
            .build()
    }

    async fn search_songs(&self, query: &str, page: Page) -> Result<SearchHits<Song>> {
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

    async fn search_albums(&self, query: &str, page: Page) -> Result<SearchHits<Album>> {
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

    async fn search_playlists(&self, query: &str, page: Page) -> Result<SearchHits<Playlist>> {
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

    async fn search_artists(&self, query: &str, page: Page) -> Result<SearchHits<Artist>> {
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

    /// 歌手详情:并发取「详情(简介/计数/热门曲)」与「粉丝数」两端点,聚合成完整 [`Artist`]。
    ///
    /// `/weapi/v1/artist/{id}` 顶层不带粉丝数,粉丝数只有 `/api/artist/follow/count/get` 给;两端点
    /// 并发打、就地聚合。详情端点失败则整体失败(主数据);粉丝数端点失败降级 0(非致命,warn 留痕)。
    async fn artist_detail(&self, id: &ArtistId) -> Result<Artist> {
        let (detail, fans) = tokio::join!(
            api::artist::detail(&self.transport, id),
            api::artist::follow_count(&self.transport, id),
        );
        let detail = detail.map_err(map_err)?;
        let fans = fans.unwrap_or_else(|e| {
            mineral_log::warn!(
                target: "netease",
                artist = id.value(),
                error = mineral_log::chain(&e),
                "artist follow count fetch failed; fans=0"
            );
            0
        });
        Ok(convert::artist_detail_to_model(detail, fans))
    }

    async fn artist_albums(&self, id: &ArtistId, page: Page) -> Result<Vec<Album>> {
        let dto = api::artist::albums(&self.transport, id, page.offset, page.limit)
            .await
            .map_err(map_err)?;
        Ok(dto
            .hot_albums
            .into_iter()
            .map(convert::artist_album_to_model)
            .collect())
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
        let dto = api::album::detail(&self.transport, id)
            .await
            .map_err(map_err)?;
        Ok(convert::album_detail_to_model(dto))
    }

    /// 歌单完整详情(元信息 + 曲目),配 persist 缓存(版本号 `trackUpdateTime` 条件刷新)。
    ///
    /// 元信息从轻量请求(同端点 `limit=0`)拿——它返回 playlist 对象(名/简介/封面/计数/
    /// 版本戳 + `trackIds` 顺序),不含 tracks 大头。曲目则:
    /// - 缓存命中且版本一致 → 本地 song_meta 按远端顺序重建,省拉上千首。
    /// - 版本变 / 无缓存 → 全拉(`limit=1000`)覆盖写回。
    /// - 轻请求失败 → 降级旧缓存曲目(元信息缺,只剩 id + 曲目),体验优先;无缓存才冒泡。
    ///
    /// 缓存只存曲目(`Vec<Song>`),元信息每次从轻请求拿,故缓存结构不必随 model 扩张。
    async fn playlist_detail(&self, id: &PlaylistId) -> Result<Playlist> {
        // 1. 轻量请求拿元信息 + 版本戳 + trackIds 顺序(limit=0,不拉 tracks)。
        let meta = match api::playlist::detail(&self.transport, id, 0).await {
            Ok(r) => r.playlist,
            Err(e) => {
                if let Some(stale) = playlist_cache::try_load_stale(&self.persist, id).await {
                    mineral_log::warn!(
                        target: "netease",
                        playlist = %id.value(),
                        error = mineral_log::chain(&e),
                        "歌单元信息轻请求失败,降级返回旧缓存曲目(元信息缺)"
                    );
                    return Ok(Playlist::builder()
                        .id(id.clone())
                        .name(String::new())
                        .songs(stale)
                        .build());
                }
                return Err(map_err(e));
            }
        };
        let track_ids = meta
            .track_ids
            .iter()
            .map(|t| t.id.to_string())
            .collect::<Vec<String>>();

        // 2. 缓存命中且版本一致 → 按远端顺序由本地重建曲目;否则全拉覆盖写回。
        let songs = if let Some(cached) = playlist_cache::try_rebuild_if_current(
            &self.persist,
            id,
            meta.track_update_time,
            &track_ids,
        )
        .await
        {
            cached
        } else {
            match api::playlist::detail(&self.transport, id, 1000).await {
                Ok(full) => {
                    let songs = full
                        .playlist
                        .tracks
                        .into_iter()
                        .map(convert::album_song_to_model)
                        .collect::<Vec<Song>>();
                    playlist_cache::store(
                        &self.persist,
                        id,
                        Some(&meta.name),
                        Some(meta.track_update_time),
                        &songs,
                    )
                    .await;
                    songs
                }
                Err(e) => {
                    if let Some(stale) = playlist_cache::try_load_stale(&self.persist, id).await {
                        mineral_log::warn!(
                            target: "netease",
                            playlist = %id.value(),
                            error = mineral_log::chain(&e),
                            "歌单曲目全拉失败,降级返回旧缓存曲目"
                        );
                        stale
                    } else {
                        return Err(map_err(e));
                    }
                }
            }
        };
        Ok(convert::playlist_info_to_model(&meta, songs))
    }

    /// 播放 URL,**双层降级**(spec §4.3):先打 v1(字符串等级),取到可播 url 即用;
    /// v1 出错或只回试听片段(映射后为空)再降级 legacy(数字 br)。
    async fn song_urls(&self, ids: &[SongId], quality: BitRate) -> Result<Vec<PlayUrl>> {
        if let Ok(dtos) = api::song::song_url_v1(&self.transport, ids, quality).await {
            let urls = convert::play_urls(dtos, quality);
            if !urls.is_empty() {
                return Ok(urls);
            }
        }
        let dtos = api::song::song_url_legacy(&self.transport, ids, quality)
            .await
            .map_err(map_err)?;
        Ok(convert::play_urls(dtos, quality))
    }

    async fn lyrics(&self, id: &SongId) -> Result<Lyrics> {
        api::lyric::lyrics(&self.transport, id)
            .await
            .map_err(map_err)
    }

    async fn login(&self, credential: Credential) -> Result<()> {
        match credential {
            Credential::Cookie(_) => {
                // 已在 transport 的 cookie jar 内;还需要触发 token 续签来确保有效。
                api::login::login_refresh(&self.transport)
                    .await
                    .map_err(map_err)
            }
            // 邮箱/手机密码登录的端点已废弃且不稳定,暂不支持;
            // 推荐用二维码或导入 cookie。
            _ => Err(Error::NotSupported),
        }
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
            .map_err(Error::Other)
    }

    /// 远端真实累计播放次数:登录(有 uid)才查回忆坐标;未登录返回 [`Error::NotSupported`]。
    async fn remote_play_count(&self, id: &SongId) -> Result<u32> {
        if self.user_id.is_none() {
            return Err(Error::NotSupported);
        }
        api::song::remote_play_count(&self.transport, id)
            .await
            .map_err(Error::Other)
    }

    async fn on_played(&self, id: &SongId, completed: bool, listen_ms: u64) -> Result<()> {
        let store = self.persist.scope(SourceKind::NETEASE);
        if completed {
            store
                .record_play(id, listen_ms)
                .await
                .map_err(Error::Other)?;
        } else {
            store.record_skip(id).await.map_err(Error::Other)?;
        }
        store
            .push_history(id, completed, listen_ms)
            .await
            .map_err(Error::Other)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::{WrapErr, eyre};
    use mineral_channel_core::{Error, MusicChannel};
    use mineral_model::{SongId, SourceKind};
    use mineral_persist::ServerStore;

    use crate::NeteaseChannel;
    use crate::config::NeteaseConfig;
    use crate::error::ApiCodeError;

    /// `map_err` 对携带 [`ApiCodeError`] 的 Report 按 code 结构化映射;
    /// 普通 Report 落 `Error::Other` 兜底。
    #[test]
    fn map_err_translates_api_codes() {
        let f = |code: i64| {
            super::map_err(color_eyre::Report::new(ApiCodeError {
                code,
                message: String::from("m"),
            }))
        };
        assert!(matches!(f(301), Error::AuthRequired));
        assert!(matches!(f(512), Error::RateLimited));
        assert!(matches!(f(502), Error::Api { code: 502, .. }));
        assert!(matches!(f(405), Error::Api { code: 405, .. }));
        assert!(matches!(
            super::map_err(eyre!("plain network-ish error")),
            Error::Other(_)
        ));
    }

    /// api 层 `.wrap_err(..)` 加过上下文后,downcast 仍沿 source 链命中,
    /// 映射不退化(防"格式化成字符串再 eyre!"一类的回归)。
    #[test]
    fn map_err_survives_wrap_err_context() -> color_eyre::Result<()> {
        let res: color_eyre::Result<()> = Err(color_eyre::Report::new(ApiCodeError {
            code: 301,
            message: String::new(),
        }));
        let e = res
            .wrap_err("fetch user playlists")
            .err()
            .ok_or_else(|| eyre!("expected err"))?;
        assert!(matches!(super::map_err(e), Error::AuthRequired));
        Ok(())
    }

    /// favorite 方法收窄为**纯远端**:匿名 channel(未登录)无远端可查/可打,
    /// `liked_song_ids` 与 `set_loved` 都返回 [`Error::NotSupported`]。
    ///
    /// 本地 favorited 集与本地写入统一由 server 经 persist 负责(不在 channel 降级),
    /// 故 channel 未登录时不再读本地 loved_ids(回归:曾在此降级读本地)。
    #[tokio::test]
    async fn favorite_methods_not_supported_when_anonymous() -> color_eyre::Result<()> {
        let config = NeteaseConfig::builder()
            .max_connections(0)
            .proxy(None)
            .timeout_secs(100)
            .build();
        let channel = NeteaseChannel::new(&config, ServerStore::disabled())?;

        assert!(
            matches!(channel.liked_song_ids().await, Err(Error::NotSupported)),
            "匿名 liked_song_ids 应 NotSupported(纯远端,不降级本地)"
        );
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

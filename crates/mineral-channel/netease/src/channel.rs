//! `impl MusicChannel for NeteaseChannel`。
//!
//! 把 `api/` 模块里的逻辑层方法绑到 `mineral_channel_core::MusicChannel` 这个 trait,
//! 让 binary 上层可以面向 trait 编程。

use async_trait::async_trait;
use color_eyre::eyre::eyre;
use isahc::cookies::{Cookie, CookieJar};
use mineral_channel_core::{ChannelCaps, Credential, Error, MusicChannel, Page, Result};
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, SearchKind,
    Song, SongId, SourceKind, UserId,
};
use mineral_persist::ServerStore;
use rustc_hash::FxHashSet;

use crate::error::ApiCodeError;

use crate::api;
use crate::config::NeteaseConfig;
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
                SearchKind::Album,
                SearchKind::Playlist,
                SearchKind::Artist,
            ])
            .playlist_edit(true)
            .song_web_url(Some("https://music.163.com/song?id={id}".to_owned()))
            .playlist_web_url(Some("https://music.163.com/playlist?id={id}".to_owned()))
            .build()
    }

    async fn search_songs(&self, query: &str, page: Page) -> Result<Vec<Song>> {
        api::search::search_songs(&self.transport, query, page.offset, page.limit)
            .await
            .map_err(map_err)
    }

    async fn search_albums(&self, query: &str, page: Page) -> Result<Vec<Album>> {
        api::search::search_albums(&self.transport, query, page.offset, page.limit)
            .await
            .map_err(map_err)
    }

    async fn search_playlists(&self, query: &str, page: Page) -> Result<Vec<Playlist>> {
        api::search::search_playlists(&self.transport, query, page.offset, page.limit)
            .await
            .map_err(map_err)
    }

    async fn search_artists(&self, query: &str, page: Page) -> Result<Vec<Artist>> {
        api::search::search_artists(&self.transport, query, page.offset, page.limit)
            .await
            .map_err(map_err)
    }

    async fn artist_detail(&self, id: &ArtistId) -> Result<Artist> {
        api::artist::artist_detail(&self.transport, id)
            .await
            .map_err(map_err)
    }

    async fn artist_albums(&self, id: &ArtistId, page: Page) -> Result<Vec<Album>> {
        api::artist::artist_albums(&self.transport, id, page.offset, page.limit)
            .await
            .map_err(map_err)
    }

    async fn create_playlist(&self, name: &str) -> Result<Playlist> {
        api::playlist_edit::create_playlist(&self.transport, name)
            .await
            .map_err(map_err)
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
        api::song::songs_detail(&self.transport, ids)
            .await
            .map_err(map_err)
    }

    async fn songs_in_album(&self, id: &AlbumId) -> Result<Vec<Song>> {
        api::album::songs_in_album(&self.transport, id)
            .await
            .map_err(map_err)
    }

    /// 歌单内全部歌曲,配 persist 缓存(版本号 `trackUpdateTime` 条件刷新,远端为准)。
    ///
    /// 先轻量拉远端版本戳 + 全量 trackIds 顺序(不拉完整 tracks):
    /// - 缓存命中且版本一致 → 由本地 song_meta 按远端顺序重建,省掉拉上千首 tracks。
    /// - 版本变 / 无缓存 / 旧缓存无版本戳 → 全拉远端覆盖并写回(含新版本戳)。
    /// - 轻请求网络失败 → 降级旧缓存(忽略版本)体验优先;无缓存才冒泡 `Err`。
    ///
    /// 缓存是优化,远端始终是事实来源:版本戳一变即全拉覆盖,命中也以远端 trackIds 顺序重建。
    async fn songs_in_playlist(&self, id: &PlaylistId) -> Result<Vec<Song>> {
        // 1. 轻量请求拿版本戳 + 全量 trackIds 顺序。
        let (remote_tut, remote_track_ids) =
            match api::playlist::playlist_version(&self.transport, id).await {
                Ok(v) => v,
                Err(e) => {
                    // 轻请求失败:降级旧缓存(忽略版本),体验优先;无缓存才冒泡。
                    if let Some(stale) = playlist_cache::try_load_stale(&self.persist, id).await {
                        mineral_log::warn!(
                            target: "netease",
                            playlist = %id.value(),
                            error = mineral_log::chain(&e),
                            "歌单版本轻请求失败,降级返回旧缓存"
                        );
                        return Ok(stale);
                    }
                    return Err(map_err(e));
                }
            };

        // 2. 缓存命中且版本一致 → 按远端顺序由本地重建,省 tracks 大头。
        if let Some(cached) =
            playlist_cache::try_rebuild_if_current(&self.persist, id, remote_tut, &remote_track_ids)
                .await
        {
            return Ok(cached);
        }

        // 3. 未命中 / 版本变更 → 全拉远端覆盖,写回含新版本戳。
        match api::playlist::songs_in_playlist(&self.transport, id).await {
            Ok(songs) => {
                playlist_cache::store(
                    &self.persist,
                    id,
                    /*name*/ None,
                    Some(remote_tut),
                    &songs,
                )
                .await;
                Ok(songs)
            }
            Err(e) => {
                // 全拉失败:仍尝试降级旧缓存,体验优先。
                if let Some(stale) = playlist_cache::try_load_stale(&self.persist, id).await {
                    mineral_log::warn!(
                        target: "netease",
                        playlist = %id.value(),
                        error = mineral_log::chain(&e),
                        "歌单远端全拉失败,降级返回旧缓存"
                    );
                    return Ok(stale);
                }
                Err(map_err(e))
            }
        }
    }

    async fn song_urls(&self, ids: &[SongId], quality: BitRate) -> Result<Vec<PlayUrl>> {
        api::song::song_urls(&self.transport, ids, quality)
            .await
            .map_err(map_err)
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
        api::playlist::user_playlists(&self.transport, uid)
            .await
            .map_err(map_err)
    }

    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        match self.user_id.as_ref() {
            Some(uid) => api::playlist::user_playlists(&self.transport, uid)
                .await
                .map_err(map_err),
            None => Err(Error::NotSupported),
        }
    }

    /// 返回当前用户喜欢的歌曲 ID 集合。
    ///
    /// **远端是事实来源**:已登录且远端拉取成功,完全返回远端结果,本地 persist 不参与。
    /// **降级**:未登录或远端 fetch 失败时,返回本地 persist 记录的 `loved_ids`
    /// (体验近似;未登录场景下本地 love 同样可见,空集也合法)。
    ///
    /// # Return:
    ///   远端或本地 persist 的 loved id 集合。
    async fn liked_song_ids(&self) -> Result<FxHashSet<SongId>> {
        // 远端是事实来源:登录且 fetch 成功则完全以远端为准
        if let Some(uid) = self.user_id.as_ref() {
            match api::user::liked_song_ids(&self.transport, uid).await {
                Ok(remote) => return Ok(remote),
                Err(e) => {
                    mineral_log::warn!(
                        target: "netease",
                        error = mineral_log::chain(&e),
                        "远端 liked 拉取失败,降级本地 persist loved"
                    );
                    // 落到下面的本地降级
                }
            }
        }
        // 远端不可用(未登录 / fetch 失败):用本地 persist loved_ids 作体验近似
        self.persist
            .scope(SourceKind::NETEASE)
            .loved_ids()
            .await
            .map_err(Error::Other)
    }

    async fn set_loved(&self, id: &SongId, loved: bool) -> Result<()> {
        // 本地是事实来源,必写(降级 persist 下自动 no-op)
        self.persist
            .scope(SourceKind::NETEASE)
            .set_loved(id, loved)
            .await
            .map_err(Error::Other)?;
        // 远端尽力:需登录;失败只 warn,不影响本地已记录的结果
        if self.user_id.is_some()
            && let Err(e) = api::song::like_song(&self.transport, id, loved).await
        {
            mineral_log::warn!(
                target: "netease",
                error = mineral_log::chain(&e),
                "远端红心失败,本地已记录"
            );
        }
        Ok(())
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

    /// 匿名 channel(未登录,`user_id = None`)调用 `liked_song_ids` 时
    /// 应降级读本地 persist 的 `loved_ids`,返回本地写入的两首 id。
    #[tokio::test]
    async fn liked_song_ids_falls_back_to_local_when_no_remote() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;

        // 写两首本地 loved
        let id_a = SongId::new(SourceKind::NETEASE, "10001");
        let id_b = SongId::new(SourceKind::NETEASE, "10002");
        let store = persist.scope(SourceKind::NETEASE);
        store.set_loved(&id_a, /*loved*/ true).await?;
        store.set_loved(&id_b, /*loved*/ true).await?;

        // 构造匿名 channel(无登录态 → 远端不会被调用)
        let config = NeteaseConfig::builder()
            .max_connections(0)
            .proxy(None)
            .timeout_secs(100)
            .build();
        let channel = NeteaseChannel::new(&config, persist)?;

        let ids = channel.liked_song_ids().await?;
        assert!(ids.contains(&id_a), "本地 loved id_a 应在降级结果中");
        assert!(ids.contains(&id_b), "本地 loved id_b 应在降级结果中");
        assert_eq!(ids.len(), 2, "降级结果只应含本地写入的两首");
        Ok(())
    }
}

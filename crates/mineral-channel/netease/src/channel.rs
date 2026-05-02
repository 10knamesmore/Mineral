//! `impl MusicChannel for NeteaseChannel`。
//!
//! 把 `api/` 模块里的逻辑层方法绑到 `mineral_channel_core::MusicChannel` 这个 trait,
//! 让 binary 上层可以面向 trait 编程。

use async_trait::async_trait;
use color_eyre::eyre::eyre;
use isahc::cookies::{Cookie, CookieJar};
use mineral_channel_core::{Credential, Error, MusicChannel, Page, Result};
use mineral_model::{
    Album, AlbumId, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, Song, SongId, SourceKind,
    UserId,
};
use rustc_hash::FxHashSet;

use crate::api;
use crate::config::NeteaseConfig;
use crate::transport::Transport;

pub struct NeteaseChannel {
    transport: Transport,

    /// 当前实例绑定的登录用户 uid;`None` 时 `my_playlists` 返回 `NotSupported`。
    user_id: Option<UserId>,
}

impl NeteaseChannel {
    pub fn new(config: &NeteaseConfig) -> color_eyre::Result<Self> {
        Ok(Self {
            transport: Transport::new(config)?,
            user_id: None,
        })
    }

    /// 仅用 `MUSIC_U` cookie 构造 channel,不绑 uid。
    ///
    /// `music_u` 通常从浏览器 `Application → Cookies → music.163.com` 复制。
    /// 这种 channel 能跑 search / 详情类端点,但 [`MusicChannel::my_playlists`]
    /// 因为不知道 uid 会返回 [`mineral_channel_core::Error::NotSupported`];
    /// 同时绑 uid 的入口走 [`Self::with_credential`]。
    pub fn with_cookie(config: &NeteaseConfig, music_u: &str) -> color_eyre::Result<Self> {
        Self::build(config, music_u, None)
    }

    /// 同时注入 `MUSIC_U` 与登录用户 uid,得到一个有「我的歌单」上下文的 channel。
    ///
    /// # Params:
    ///   - `config`: HTTP 客户端配置
    ///   - `music_u`: 网易云核心登录 cookie 值
    ///   - `user_id`: 登录用户 uid(`my_playlists` 内部转发给 `user_playlists`)
    pub fn with_credential(
        config: &NeteaseConfig,
        music_u: &str,
        user_id: UserId,
    ) -> color_eyre::Result<Self> {
        Self::build(config, music_u, Some(user_id))
    }

    fn build(
        config: &NeteaseConfig,
        music_u: &str,
        user_id: Option<UserId>,
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
        })
    }

    /// 暴露内部 transport,给一些不在 `MusicChannel` 范围内的端点用
    /// (例如二维码登录 GetKey/CheckQR、ping 等)。
    pub fn transport(&self) -> &Transport {
        &self.transport
    }
}

fn map_err(e: color_eyre::Report) -> Error {
    Error::Other(e)
}

#[async_trait]
impl MusicChannel for NeteaseChannel {
    fn source(&self) -> SourceKind {
        SourceKind::Netease
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

    async fn songs_in_playlist(&self, id: &PlaylistId) -> Result<Vec<Song>> {
        api::playlist::songs_in_playlist(&self.transport, id)
            .await
            .map_err(map_err)
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

    async fn liked_song_ids(&self) -> Result<FxHashSet<SongId>> {
        match self.user_id.as_ref() {
            Some(uid) => api::user::liked_song_ids(&self.transport, uid)
                .await
                .map_err(map_err),
            None => Err(Error::NotSupported),
        }
    }
}

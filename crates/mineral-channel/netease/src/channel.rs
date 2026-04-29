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

use crate::api;
use crate::config::NeteaseConfig;
use crate::transport::Transport;

pub struct NeteaseChannel {
    transport: Transport,
}

impl NeteaseChannel {
    pub fn new(config: &NeteaseConfig) -> color_eyre::Result<Self> {
        Ok(Self {
            transport: Transport::new(config)?,
        })
    }

    /// 用 `MUSIC_U` cookie 字符串构造一个已登录的 channel。
    ///
    /// `music_u` 通常从浏览器 `Application → Cookies → music.163.com` 复制。
    pub fn with_cookie(config: &NeteaseConfig, music_u: &str) -> color_eyre::Result<Self> {
        let jar = CookieJar::new();
        let url = "https://music.163.com".parse().unwrap();
        let cookie = Cookie::builder("MUSIC_U", music_u)
            .domain("music.163.com")
            .path("/")
            .build()
            .map_err(|e| eyre!("build cookie: {e}"))?;
        jar.set(cookie, &url)
            .map_err(|e| eyre!("set cookie: {e}"))?;
        Ok(Self {
            transport: Transport::from_cookie_jar(config, jar)?,
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
}

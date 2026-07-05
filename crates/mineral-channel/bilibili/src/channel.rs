//! `impl MusicChannel for BilibiliChannel`。
//!
//! 业务层:组合 `api/` 端点(协议 → DTO)与 `convert`(DTO → mineral-model),收敛错误为
//! `mineral_channel_core::Error`。B站取流需先经 view 定位分 P 的 cid,再打 playurl,故
//! `song_urls` 每首两跳(view + playurl);详情/搜索是单跳。

use async_trait::async_trait;
use mineral_channel_core::{ChannelCaps, Error, MusicChannel, Page, Result};
use mineral_model::{
    Album, AlbumId, BitRate, PlayUrl, Playlist, PlaylistId, SearchKind, Song, SongId, SourceKind,
    UserId,
};
use rustc_hash::FxHashSet;

use crate::api;
use crate::config::BilibiliConfig;
use crate::convert;
use crate::credential::StoredBilibiliAuth;
use crate::error::ApiCodeError;
use crate::transport::Transport;
use crate::wire::view::VideoInfo;

/// 哔哩哔哩 channel 实例。guest 模式(无登录)可搜索 / 详情 / 取流;带登录态(见
/// [`Self::with_credential`])额外解锁「我的收藏夹」+ 高码率。
pub struct BilibiliChannel {
    /// B站请求的 HTTP 传输层(isahc + WBI 签名 + buvid3 冷启动)。
    transport: Transport,

    /// 登录用户 mid;`None`(guest)时 `my_playlists` 返回 `NotSupported`。
    user_id: Option<UserId>,
}

impl BilibiliChannel {
    /// 构造 guest channel(公开端点:搜索 / 详情 / 取流;未登录音质封顶 ~192k)。
    ///
    /// # Params:
    ///   - `config`: B站源配置(超时 / 代理 / 并发)
    ///
    /// # Return:
    ///   channel 实例;transport 构建失败时 `Err`。
    pub fn new(config: &BilibiliConfig) -> color_eyre::Result<Self> {
        Ok(Self {
            transport: Transport::new(config)?,
            user_id: None,
        })
    }

    /// 用登录凭证构造:解锁「我的收藏夹」(mid 来自凭证)+ 高码率取流。
    ///
    /// # Params:
    ///   - `config`: B站源配置
    ///   - `auth`: 登录凭证三件套
    ///
    /// # Return:
    ///   带登录态的 channel 实例;transport 构建失败时 `Err`。
    pub fn with_credential(
        config: &BilibiliConfig,
        auth: &StoredBilibiliAuth,
    ) -> color_eyre::Result<Self> {
        Ok(Self {
            transport: Transport::from_credential(config, auth)?,
            user_id: Some(UserId::new(SourceKind::BILIBILI, auth.dede_user_id.clone())),
        })
    }

    /// 暴露内部 transport,给不在 `MusicChannel` 范围内的端点用(二维码登录 generate/poll)。
    pub fn transport(&self) -> &Transport {
        &self.transport
    }
}

/// api 层 `color_eyre::Report` 收敛到 channel-core 错误。
///
/// 携带 [`ApiCodeError`] 的按 code 映射:`-101`(未登录)→ `AuthRequired`、`-352`(风控/签名
/// 失效)→ `RateLimited`、其余透传 `Api`;纯网络/解析类 Report 落 `Other`。
fn map_err(e: color_eyre::Report) -> Error {
    match e.downcast_ref::<ApiCodeError>() {
        Some(api) => match api.code {
            -101 => Error::AuthRequired,
            -352 => Error::RateLimited,
            _ => Error::Api {
                code: api.code,
                message: api.message.clone(),
            },
        },
        None => Error::Other(e),
    }
}

/// [`Page`] 的 offset/limit → B站页码(1 起);`limit == 0` 退回第 1 页。
fn page_number(page: Page) -> u32 {
    page.offset.checked_div(page.limit).map_or(1, |q| q + 1)
}

/// 解析 SongId 的裸值 `bvid:page` → `(bvid, page)`;格式不符返回 `None`。
fn parse_song_ref(id: &SongId) -> Option<(String, i32)> {
    let (bvid, page) = id.as_str().rsplit_once(':')?;
    let page = page.parse::<i32>().ok()?;
    Some((bvid.to_owned(), page))
}

/// 在视频详情里定位某分 P 的 cid:优先 `pages` 里 `page` 匹配项;单 P(无 pages)且
/// `page <= 1` 用顶层 cid 兜底。
fn cid_for_page(info: &VideoInfo, page: i32) -> Option<i64> {
    if let Some(pages) = &info.pages
        && let Some(p) = pages.iter().find(|p| p.page == page)
    {
        return Some(p.cid);
    }
    if page <= 1 { Some(info.cid) } else { None }
}

#[async_trait]
impl MusicChannel for BilibiliChannel {
    fn source(&self) -> SourceKind {
        SourceKind::BILIBILI
    }

    fn caps(&self) -> ChannelCaps {
        // MVP 只全库搜视频(→ Song);收藏夹(歌单)stage 2 接,写操作永不支持(只读源)。
        // song_web_url 暂空:裸 id 是 `bvid:page`,套不进单一 `{id}` 视频模板。
        ChannelCaps::builder()
            .searchable(vec![SearchKind::Song])
            .playlist_edit(false)
            .build()
    }

    async fn search_songs(&self, query: &str, page: Page) -> Result<Vec<Song>> {
        api::search::search_songs(&self.transport, query, page_number(page))
            .await
            .map_err(map_err)
    }

    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>> {
        // 按 bvid 去重取 view,产出各 P 的 Song,再过滤回请求的 id(同视频不重复取 view)。
        let wanted: FxHashSet<String> = ids.iter().map(|i| i.as_str().to_owned()).collect();
        let mut bvids = Vec::<String>::new();
        for id in ids {
            if let Some((bvid, _)) = parse_song_ref(id)
                && !bvids.contains(&bvid)
            {
                bvids.push(bvid);
            }
        }
        let mut out = Vec::<Song>::new();
        for bvid in bvids {
            let info = api::view::video_info(&self.transport, &bvid)
                .await
                .map_err(map_err)?;
            for song in convert::view_to_album(info).songs {
                if wanted.contains(song.id.as_str()) {
                    out.push(song);
                }
            }
        }
        Ok(out)
    }

    async fn album_detail(&self, id: &AlbumId) -> Result<Album> {
        let info = api::view::video_info(&self.transport, id.as_str())
            .await
            .map_err(map_err)?;
        Ok(convert::view_to_album(info))
    }

    async fn song_urls(&self, ids: &[SongId], _quality: BitRate) -> Result<Vec<PlayUrl>> {
        // 每首两跳:view 定位分 P cid → playurl 取 dash.audio → convert 带上 Referer 取流头。
        // 音质档由 B站按登录态定(guest 封顶),故 `quality` 暂不下发。
        let mut out = Vec::<PlayUrl>::new();
        for id in ids {
            let Some((bvid, page)) = parse_song_ref(id) else {
                continue;
            };
            let info = api::view::video_info(&self.transport, &bvid)
                .await
                .map_err(map_err)?;
            let Some(cid) = cid_for_page(&info, page) else {
                continue;
            };
            let result = api::playurl::playurl(&self.transport, &bvid, cid)
                .await
                .map_err(map_err)?;
            if let Some(pu) = convert::playurl_to_play(id.clone(), result) {
                out.push(pu);
            }
        }
        Ok(out)
    }

    async fn playlist_detail(&self, id: &PlaylistId) -> Result<Playlist> {
        // 收藏夹内容:翻页拉全条目 → Song,配元信息成 Playlist(缺 info 时以 fid 兜底名)。
        let fid = id.as_str();
        let list = api::fav::all_resources(&self.transport, fid)
            .await
            .map_err(map_err)?;
        let songs = list
            .medias
            .unwrap_or_default()
            .into_iter()
            .filter_map(convert::fav_media_to_song)
            .collect::<Vec<Song>>();
        match list.info {
            Some(info) => Ok(convert::fav_list_to_playlist(fid, info, songs)),
            None => Ok(Playlist::builder()
                .id(id.clone())
                .name(fid.to_owned())
                .songs(songs)
                .build()),
        }
    }

    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        // 需登录(mid 来自凭证);guest 无 mid → NotSupported(上层视为该源不贡献歌单)。
        let Some(uid) = self.user_id.as_ref() else {
            return Err(Error::NotSupported);
        };
        let folders = api::fav::created_folders(&self.transport, uid.as_str())
            .await
            .map_err(map_err)?;
        Ok(folders
            .list
            .unwrap_or_default()
            .into_iter()
            .map(convert::fav_folder_to_playlist)
            .collect())
    }
}

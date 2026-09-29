//! `impl MusicChannel for BilibiliChannel`。
//!
//! 业务层:组合 `api/` 端点(协议 → DTO)与 `convert`(DTO → mineral-model),收敛错误为
//! `mineral_channel_core::Error`。B站取流需先经 view 定位分 P 的 cid,再打 playurl,故
//! Playback provider resolve 每首两跳(view + playurl);详情/搜索是单跳。

use async_trait::async_trait;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, Error, MusicChannel, Page, PageResult, Result,
};
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, Playlist, PlaylistId, SearchKind, Song, SongId, SourceKind,
    UserId,
};
use mineral_playback::{
    DirectPreparedPlayback, Error as PlaybackError, PlaybackProvider, PlaybackRequest,
    PreparedPlayback,
};
use rustc_hash::FxHashSet;
use tokio_util::sync::CancellationToken;

use crate::api;
use crate::config::BilibiliConfig;
use crate::convert;
use crate::credential::StoredBilibiliAuth;
use crate::error::{ApiCodeError, Error as BilibiliError};
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
    pub fn new(config: &BilibiliConfig) -> crate::Result<Self> {
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
    ) -> crate::Result<Self> {
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

/// 将 B站领域错误映射为跨 channel 分类，保留原始 cause。
fn map_err(error: BilibiliError) -> Error {
    match error {
        BilibiliError::Api(ApiCodeError { code: -101, .. }) => Error::AuthRequired,
        BilibiliError::Api(ApiCodeError { code: -352, .. }) => Error::RateLimited,
        BilibiliError::Api(ApiCodeError { code, message }) => Error::Api { code, message },
        BilibiliError::Network { .. }
        | BilibiliError::Cookie { .. }
        | BilibiliError::SigningClock(_)
        | BilibiliError::SigningKeys { .. } => Error::Network {
            source: Box::new(error),
        },
        BilibiliError::Parse { .. } | BilibiliError::Serialize(_) => Error::Parse {
            source: Box::new(error),
        },
        BilibiliError::InvalidData { field } => Error::InvalidData { field },
        BilibiliError::Path(_)
        | BilibiliError::File { .. }
        | BilibiliError::InvalidCredentialPath { .. } => Error::Storage {
            source: Box::new(error),
        },
        BilibiliError::Qr(_)
        | BilibiliError::LoginCancelled
        | BilibiliError::LoginExpired
        | BilibiliError::LoginStatus { .. } => Error::Parse {
            source: Box::new(error),
        },
    }
}

/// [`Page`] 的 offset/limit → B站页码(1 起);`limit == 0` 退回第 1 页。
///
/// 依赖调用方的 offset 是 limit 的整数倍(翻页游标页对齐)。别按「实际返回条数」累加
/// offset——B站每页实际条数与 limit 无关,余数会让页号折回/跳页。
fn page_number(page: Page) -> u32 {
    page.offset.checked_div(page.limit).map_or(1, |q| q + 1)
}

/// 解析 SongId 的裸值 `bvid:page` → `(bvid, page)`;格式不符返回 `None`。
fn parse_song_ref(id: &SongId) -> Option<(String, i32)> {
    let (bvid, page) = id.as_str().rsplit_once(':')?;
    let page = page.parse::<i32>().ok()?;
    Some((bvid.to_owned(), page))
}

/// 多 P 展开时单条 view 失败是否应中止整个 `playlist_detail`(而非降级为 unavailable 单行)。
///
/// 非 API 故障曾通过 `Other` 中止整夹；现在网络、解析、存储及无效响应仍必须中止。
/// 只有远端 API 返回的视频自身内容错误可降级单行。
fn expansion_should_abort(err: &Error) -> bool {
    !matches!(err, Error::Api { .. })
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
        // 全库搜视频(BV → Album)+ 用户(→ Artist);写操作永不支持(只读源)。B站一个视频
        // 就是一张专辑,分 P 是曲目,搜索直接产 Album、不投影成 P1 单曲(避免「显示全 BV 总长、
        // 实播单 P」的时长错位)。song 模板用位置占位拆 `bvid:page` 复合裸 id(`?p=1` 对单 P
        // 视频冗余但合法,不特判);收藏夹的稳定分享 URL 需要 mid 参与,裸 fid 拼不出,歌单模板留空。
        ChannelCaps::builder()
            .searchable(vec![SearchKind::Album, SearchKind::Artist])
            .playlist_edit(false)
            // UP 主详情:只有投稿专辑区,无「热门曲」区(B站无整源热门单曲概念,见 artist_detail)。
            .artist_sections(ArtistSections::new(vec![ArtistSectionKind::Albums]))
            // album = 整个视频(裸 id 即 bvid,无分 P 段),用 `{id}` 整段;artist = UP 主空间页,
            // 裸 id 即 mid。
            .song_web_url(Some("https://www.bilibili.com/video/{0}?p={1}".to_owned()))
            .album_web_url(Some("https://www.bilibili.com/video/{id}".to_owned()))
            .artist_web_url(Some("https://space.bilibili.com/{id}".to_owned()))
            .build()
    }

    async fn search_albums(&self, query: &str, page: Page) -> Result<PageResult<Album>> {
        api::search::search_albums(&self.transport, query, page_number(page), page.limit)
            .await
            .map_err(map_err)
    }

    async fn search_artists(&self, query: &str, page: Page) -> Result<PageResult<Artist>> {
        api::search::search_artists(&self.transport, query, page_number(page), page.limit)
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
            for track in convert::view_to_album(info).tracks {
                let song = track.song;
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

    async fn artist_detail(&self, id: &ArtistId) -> Result<Artist> {
        // 只取名片(基本信息);B站没有「整源热门单曲」概念,不投影投稿当热门曲——UP 主的投稿
        // 专辑经 artist_albums 单独拉,UI 据 caps.artist_top_songs=false 只显示 Albums 区。
        let card = api::space::card(&self.transport, id.as_str())
            .await
            .map_err(map_err)?;
        Ok(convert::card_to_artist(id.clone(), card))
    }

    async fn artist_albums(&self, id: &ArtistId, page: Page) -> Result<PageResult<Album>> {
        // UP 主投稿,每 BV 一张专辑(元信息版,P 数/曲目详情走 album_detail)。
        let result = api::space::arc_videos(
            &self.transport,
            id.as_str(),
            api::space::ArcOrder::Pubdate,
            page_number(page),
            page.limit,
        )
        .await
        .map_err(map_err)?;
        Ok(convert::arc_videos_to_albums(result, page))
    }

    async fn playlist_detail(
        &self,
        id: &PlaylistId,
        _load: mineral_channel_core::PlaylistLoad,
    ) -> Result<mineral_channel_core::PlaylistDetail> {
        // 自身内容业务码可降级为 unavailable 单行；解析、无效响应及全局故障
        // 必须中止整夹，避免将系统性故障伪装成一批失效视频。
        let fid = id.as_str();
        let list = api::fav::all_resources(&self.transport, fid)
            .await
            .map_err(map_err)?;
        let mut songs = Vec::<Song>::new();
        for media in list.medias.unwrap_or_default() {
            let Some(plan) = convert::plan_fav_entry(media) else {
                continue;
            };
            match plan {
                convert::FavEntryPlan::Single(song) => songs.push(song),
                convert::FavEntryPlan::Expand { bvid, fallback } => {
                    match api::view::video_info(&self.transport, &bvid).await {
                        Ok(info) => songs.extend(
                            convert::view_to_album(info)
                                .tracks
                                .into_iter()
                                .map(|track| track.song),
                        ),
                        Err(e) => {
                            let err = map_err(e);
                            if expansion_should_abort(&err) {
                                return Err(err);
                            }
                            mineral_log::warn!(
                                target: "bilibili",
                                bvid,
                                error = mineral_log::chain(&err),
                                "多 P 收藏条目返回内容业务错误,降级为单行"
                            );
                            songs.push(fallback);
                        }
                    }
                }
            }
        }
        let playlist = match list.info {
            Some(info) => convert::fav_list_to_playlist(fid, info, songs),
            None => Playlist::builder()
                .id(id.clone())
                .name(fid.to_owned())
                .entries(mineral_model::PlaylistEntry::enumerate(songs))
                .build(),
        };
        Ok(mineral_channel_core::PlaylistDetail::complete(playlist))
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

#[async_trait]
impl PlaybackProvider for BilibiliChannel {
    fn source(&self) -> SourceKind {
        SourceKind::BILIBILI
    }

    /// Resolves one Bilibili page into its best available audio track.
    async fn resolve(
        &self,
        request: PlaybackRequest,
        cancellation: CancellationToken,
    ) -> mineral_playback::Result<Box<dyn PreparedPlayback>> {
        if cancellation.is_cancelled() {
            return Err(PlaybackError::Cancelled);
        }
        let (bvid, page) =
            parse_song_ref(request.song_id()).ok_or_else(|| PlaybackError::Provider {
                source: Box::new(BilibiliError::InvalidData {
                    field: "Bilibili song id",
                }),
            })?;
        let info = api::view::video_info(&self.transport, &bvid)
            .await
            .map_err(|source| PlaybackError::Provider {
                source: Box::new(source),
            })?;
        let cid = cid_for_page(&info, page).ok_or_else(|| PlaybackError::Provider {
            source: Box::new(BilibiliError::InvalidData {
                field: "Bilibili page cid",
            }),
        })?;
        if cancellation.is_cancelled() {
            return Err(PlaybackError::Cancelled);
        }
        let result = api::playurl::playurl(&self.transport, &bvid, cid)
            .await
            .map_err(|source| PlaybackError::Provider {
                source: Box::new(source),
            })?;
        let media =
            convert::playurl_to_media(request.song_id().clone(), result).ok_or_else(|| {
                PlaybackError::Provider {
                    source: Box::new(BilibiliError::InvalidData {
                        field: "Bilibili audio track",
                    }),
                }
            })?;
        Ok(DirectPreparedPlayback::boxed(media))
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::Error;

    use super::{cid_for_page, expansion_should_abort};
    use crate::error::{ApiCodeError, Error as BilibiliError};
    use crate::wire::de::from_value;
    use crate::wire::view::VideoInfo;

    /// 登录与风控 code 保持 channel 的既有重试分类。
    #[test]
    fn maps_api_codes_without_losing_other_causes() {
        let api = |code| {
            super::map_err(BilibiliError::Api(ApiCodeError {
                code,
                message: Some("remote error".to_owned()),
            }))
        };
        assert!(matches!(api(-101), Error::AuthRequired));
        assert!(matches!(api(-352), Error::RateLimited));
        assert!(matches!(api(-404), Error::Api { code: -404, .. }));
        let network = super::map_err(BilibiliError::Network {
            operation: "send request",
            source: Box::new(std::io::Error::other("offline")),
        });
        assert!(matches!(&network, Error::Network { .. }));
        assert!(std::error::Error::source(&network).is_some());
        let signing_keys = super::map_err(BilibiliError::SigningKeys {
            source: Box::new(BilibiliError::InvalidData {
                field: "nav.wbi_img.img_url",
            }),
        });
        assert!(matches!(signing_keys, Error::Network { .. }));
    }

    /// 多 P 展开时，只有视频业务码能降级；网络、解析、存储故障中止整夹。
    #[test]
    fn expansion_aborts_on_non_api_failures() {
        assert!(
            expansion_should_abort(&Error::AuthRequired),
            "登录失效会命中每条,应中止"
        );
        assert!(
            expansion_should_abort(&Error::RateLimited),
            "风控会命中每条,应中止"
        );
        assert!(
            expansion_should_abort(&Error::Network {
                source: Box::new(std::io::Error::other("timeout"))
            }),
            "网络故障应中止"
        );
        assert!(
            expansion_should_abort(&Error::Storage {
                source: Box::new(std::io::Error::other("disk"))
            }),
            "存储错误应中止整夹"
        );
        assert!(
            !expansion_should_abort(&Error::Api {
                code: -404,
                message: None
            }),
            "该视频删除(API 码)只降级单行"
        );
        assert!(
            expansion_should_abort(&Error::Parse {
                source: Box::new(std::io::Error::other("bad json"))
            }),
            "解析错误应中止整夹"
        );
        assert!(
            expansion_should_abort(&Error::InvalidData { field: "view.code" }),
            "无效响应应中止整夹"
        );
    }

    /// 多 P 视频按 page 号定位 cid:命中 `pages` 里对应 page 的 cid。
    #[test]
    fn cid_for_page_locates_matching_page() -> color_eyre::Result<()> {
        let info: VideoInfo = from_value(serde_json::json!({
            "bvid": "BV1xx", "title": "多P", "cid": 1001, "duration": 440,
            "owner": { "mid": 1, "name": "甲" },
            "pages": [
                { "cid": 1001, "page": 1, "part": "一", "duration": 240 },
                { "cid": 1002, "page": 2, "part": "二", "duration": 200 }
            ]
        }))?;
        assert_eq!(cid_for_page(&info, 2), Some(1002));
        Ok(())
    }

    /// 单 P 视频(无 pages)`page <= 1` 用顶层 cid 兜底;越界 page 定位失败。
    #[test]
    fn cid_for_page_falls_back_to_top_level_for_single_page() -> color_eyre::Result<()> {
        let info: VideoInfo = from_value(serde_json::json!({
            "bvid": "BV1yy", "title": "单P", "cid": 2001, "duration": 100,
            "owner": { "mid": 7, "name": "乙" }
        }))?;
        assert_eq!(cid_for_page(&info, 1), Some(2001));
        assert_eq!(cid_for_page(&info, 5), None);
        Ok(())
    }
}

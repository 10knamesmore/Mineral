//! B站原生 DTO → `mineral_model` 类型的转换 helper。
//!
//! 内容层级映射:分 P → [`Song`],视频(BV)→ [`Album`],单 P 视频即一首歌。SongId 用
//! `{bvid}:{page}` 形态(全局唯一;裸值喂后端时按 `:` 拆回 bvid + 分 P 号)。

// convert 函数会由 channel 层调用,但本 stage 只落数据层 + 离线测试,尚无非测试调用点。
#![allow(dead_code)]

use mineral_model::{
    Album, AlbumId, AlbumRef, ArtistId, ArtistRef, AudioFormat, BitRate, MediaUrl, PlayUrl,
    Playlist, PlaylistId, Song, SongId, SourceKind,
};

use crate::wire::fav::{FavFolder, FavInfo, FavMedia};
use crate::wire::playurl::{DashAudio, PlayUrlResult};
use crate::wire::search::SearchVideoItem;
use crate::wire::view::{VideoInfo, VideoOwner, VideoPage};

/// 去掉标题里的 `<em ...>` / `</em>` 高亮标签(B站搜索给命中词裹上的 keyword 标记)。
///
/// 非 em 的其它标签原样保留(B站标题正文极少含 `<`,保守起见不当 HTML 处理)。
fn strip_em(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(lt) = rest.find('<') {
        let (Some(before), Some(from_lt)) = (rest.get(..lt), rest.get(lt..)) else {
            break;
        };
        out.push_str(before);
        match from_lt.find('>') {
            Some(gt) => {
                let tag = from_lt.get(..=gt).unwrap_or_default();
                if !is_em_tag(tag) {
                    out.push_str(tag);
                }
                rest = from_lt.get(gt + 1..).unwrap_or_default();
            }
            None => {
                out.push_str(from_lt);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 判断一段 `<...>` 是否是 `<em>` / `</em>`(含带属性的开标签)。
fn is_em_tag(tag: &str) -> bool {
    let inner = tag.trim_start_matches('<').trim_end_matches('>').trim();
    let inner = inner.strip_prefix('/').unwrap_or(inner);
    inner == "em" || inner.starts_with("em ") || inner.starts_with("em\t")
}

/// `"mm:ss"`(或 `"h:mm:ss"`)→ 毫秒。任一段非数字则整体判为 0。
fn parse_duration_ms(text: &str) -> u64 {
    let mut secs: u64 = 0;
    for part in text.split(':') {
        match part.trim().parse::<u64>() {
            Ok(n) => secs = secs.saturating_mul(60).saturating_add(n),
            Err(_) => return 0,
        }
    }
    secs.saturating_mul(1000)
}

/// B站封面 URL → [`MediaUrl::Remote`]:协议相对(`//host/...`)补 `https:`,空串 → `None`。
fn cover_media_url(pic: &str) -> Option<MediaUrl> {
    if pic.is_empty() {
        return None;
    }
    let full = pic
        .strip_prefix("//")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| pic.to_owned());
    MediaUrl::remote(&full).ok()
}

/// UP 主 → [`ArtistRef`](mid 入 BILIBILI namespace)。
fn owner_artist_ref(owner: &VideoOwner) -> ArtistRef {
    ArtistRef {
        id: ArtistId::new(SourceKind::BILIBILI, owner.mid.to_string()),
        name: owner.name.clone(),
    }
}

/// 搜索结果视频项 → [`Song`](用 P1 代表)。
///
/// 缺 `bvid` 无法定位,返回 `None`。标题 strip 高亮标签、时长 `"mm:ss"` 解析成毫秒、
/// 封面补 `https`;album 落该视频的 BV 专辑、artist 落 UP 主(mid 缺时退 `0`)。
pub(crate) fn search_video_to_song(item: SearchVideoItem) -> Option<Song> {
    let bvid = item.bvid?;
    let title = item.title.as_deref().map(strip_em).unwrap_or_default();
    let duration_ms = item.duration.as_deref().map(parse_duration_ms).unwrap_or(0);
    let cover = item.pic.as_deref().and_then(cover_media_url);
    let artists = item
        .author
        .map(|name| {
            vec![ArtistRef {
                id: ArtistId::new(
                    SourceKind::BILIBILI,
                    item.mid.unwrap_or_default().to_string(),
                ),
                name,
            }]
        })
        .unwrap_or_default();
    let album = AlbumRef {
        id: AlbumId::new(SourceKind::BILIBILI, bvid.clone()),
        name: title.clone(),
    };
    Some(
        Song::builder()
            .id(SongId::new(SourceKind::BILIBILI, format!("{bvid}:1")))
            .name(title)
            .artists(artists)
            .album(Some(album))
            .duration_ms(duration_ms)
            .cover_url(cover)
            .build(),
    )
}

/// 视频里的一个分 P → [`Song`](单 P = 一首曲目)。
///
/// # Params:
///   - `bvid`: 所属视频 BV 号(拼进 SongId 与 AlbumRef)
///   - `video_title`: 视频标题(作 album 名)
///   - `owner`: UP 主(作 artist)
///   - `pic`: 视频封面 URL(协议相对会补 https)
///   - `page`: 分 P 元信息(`part` 作曲名、`page` 拼进 SongId、`duration` 秒 → ms)
///
/// # Return:
///   该 P 对应的 [`Song`],id 为 `{bvid}:{page}`。
pub(crate) fn view_page_to_song(
    bvid: &str,
    video_title: &str,
    owner: &VideoOwner,
    pic: Option<&str>,
    page: &VideoPage,
) -> Song {
    let duration_ms = u64::try_from(page.duration)
        .unwrap_or(0)
        .saturating_mul(1000);
    Song::builder()
        .id(SongId::new(
            SourceKind::BILIBILI,
            format!("{bvid}:{}", page.page),
        ))
        .name(page.part.clone())
        .artists(vec![owner_artist_ref(owner)])
        .album(Some(AlbumRef {
            id: AlbumId::new(SourceKind::BILIBILI, bvid.to_owned()),
            name: video_title.to_owned(),
        }))
        .duration_ms(duration_ms)
        .cover_url(pic.and_then(cover_media_url))
        .build()
}

/// 视频详情 → [`Album`]:每个分 P 一首 [`Song`]。
///
/// `pages` 缺失 / 为空时(纯单 P 视频)合成一个 P1(`part` = 视频标题、时长取顶层 `duration`),
/// 保证单 P 视频也得到 1 首曲目。`pubdate`(秒)→ `publish_time_ms`(毫秒)。
pub(crate) fn view_to_album(info: VideoInfo) -> Album {
    let VideoInfo {
        bvid,
        title,
        desc,
        pic,
        duration,
        pubdate,
        owner,
        pages,
        cid,
        ..
    } = info;
    let cover = pic.as_deref().and_then(cover_media_url);
    let pages = match pages {
        Some(pages) if !pages.is_empty() => pages,
        _ => vec![VideoPage {
            cid,
            page: 1,
            part: title.clone(),
            duration: duration.unwrap_or(0),
        }],
    };
    let songs = pages
        .iter()
        .map(|page| view_page_to_song(&bvid, &title, &owner, pic.as_deref(), page))
        .collect::<Vec<Song>>();
    let track_count = u64::try_from(songs.len()).unwrap_or(0);
    Album::builder()
        .id(AlbumId::new(SourceKind::BILIBILI, bvid))
        .name(title)
        .artists(vec![owner_artist_ref(&owner)])
        .description(desc.unwrap_or_default())
        .publish_time_ms(pubdate.map(|s| s.saturating_mul(1000)).unwrap_or(0))
        .track_count(track_count)
        .cover_url(cover)
        .songs(songs)
        .build()
}

/// 取流必带的请求头:B站音频 `baseUrl` 播放要 `Referer`(否则 403),随 `PlayUrl` 经 IPC 穿透到
/// 播放/下载层(见 `stream_headers` 通道)。
fn playback_stream_headers() -> Vec<(String, String)> {
    vec![("Referer".to_owned(), "https://www.bilibili.com".to_owned())]
}

/// playurl DTO → [`PlayUrl`];无可用音频轨返回 `None`。
///
/// 选轨:有无损(flac)优先取无损,否则取 `id` 最大的普通音频轨。音质/格式按 `id` + `codecs`
/// 归一化;url 带上取流 [`playback_stream_headers`]。
///
/// # Params:
///   - `song_id`: 目标分 P 的 SongId(`{bvid}:{page}`)
///   - `result`: playurl 响应的 `data`
///
/// # Return:
///   可播的 [`PlayUrl`];无音频轨 / url 解析失败为 `None`。
pub(crate) fn playurl_to_play(song_id: SongId, result: PlayUrlResult) -> Option<PlayUrl> {
    let dash = result.dash?;
    let flac_audio = dash.flac.and_then(|f| f.audio);
    let best = best_audio(flac_audio, dash.audio.unwrap_or_default())?;
    let (quality, format) = classify_audio(best.id, best.codecs.as_deref());
    let url = MediaUrl::remote(&best.base_url).ok()?;
    Some(PlayUrl {
        song_id,
        url,
        bitrate_bps: best
            .bandwidth
            .and_then(|b| u32::try_from(b).ok())
            .unwrap_or(0),
        quality,
        size: 0,
        format,
        bit_depth: None,
        stream_headers: playback_stream_headers(),
    })
}

/// 选最优音频轨:无损轨(若有)优先,否则普通轨里取 `id` 最大(质量最高)。
fn best_audio(flac: Option<DashAudio>, mut normal: Vec<DashAudio>) -> Option<DashAudio> {
    if let Some(f) = flac {
        return Some(f);
    }
    normal.sort_by_key(|a| a.id);
    normal.pop()
}

/// 音质码 + codecs → 归一化的 (音质, 格式)。
///
/// codecs 含 `flac` 判无损;否则按 `id` 映射三档(`30216`/`30232` → Standard/Higher,其余高档
/// 归 Exhigh),格式恒 AAC(B站 dash 普通音频轨是 m4a/aac)。
fn classify_audio(id: i64, codecs: Option<&str>) -> (BitRate, AudioFormat) {
    if codecs.is_some_and(|c| c.to_ascii_lowercase().contains("flac")) {
        return (BitRate::Lossless, AudioFormat::Flac);
    }
    let quality = match id {
        30216 => BitRate::Standard,
        30232 => BitRate::Higher,
        _ => BitRate::Exhigh,
    };
    (quality, AudioFormat::Aac)
}

/// 收藏夹 folder → [`Playlist`](元信息;曲目按需走 `playlist_detail`)。
///
/// 封面 / 简介来自分页 `created/list` 端点(`list-all` 不返 cover,列表会全回退占位)。
pub(crate) fn fav_folder_to_playlist(folder: FavFolder) -> Playlist {
    Playlist::builder()
        .id(PlaylistId::new(SourceKind::BILIBILI, folder.id.to_string()))
        .name(folder.title)
        .description(folder.intro.unwrap_or_default())
        .cover_url(folder.cover.as_deref().and_then(cover_media_url))
        .track_count(u64::try_from(folder.media_count).unwrap_or(0))
        .build()
}

/// 收藏夹条目(视频)→ [`Song`](用 P1 代表);缺 bvid 返回 `None`。
///
/// 与搜索项类似,但收藏夹条目标题不含高亮标签、UP 主在 `upper` 字段。
pub(crate) fn fav_media_to_song(media: FavMedia) -> Option<Song> {
    let bvid = media.bvid?;
    let title = media.title.unwrap_or_default();
    let duration_ms = media
        .duration
        .and_then(|s| u64::try_from(s).ok())
        .unwrap_or(0)
        .saturating_mul(1000);
    let cover = media.cover.as_deref().and_then(cover_media_url);
    let artist = ArtistRef {
        id: ArtistId::new(SourceKind::BILIBILI, media.upper.mid.to_string()),
        name: media.upper.name,
    };
    let album = AlbumRef {
        id: AlbumId::new(SourceKind::BILIBILI, bvid.clone()),
        name: title.clone(),
    };
    Some(
        Song::builder()
            .id(SongId::new(SourceKind::BILIBILI, format!("{bvid}:1")))
            .name(title)
            .artists(vec![artist])
            .album(Some(album))
            .duration_ms(duration_ms)
            .cover_url(cover)
            .build(),
    )
}

/// 收藏夹详情(元信息 + 已解析曲目)→ [`Playlist`]。
///
/// # Params:
///   - `fid`: 收藏夹 id(响应 `info` 里不带,由调用方从请求透传)
///   - `info`: 收藏夹元信息
///   - `songs`: 已解析的曲目
pub(crate) fn fav_list_to_playlist(fid: &str, info: FavInfo, songs: Vec<Song>) -> Playlist {
    Playlist::builder()
        .id(PlaylistId::new(SourceKind::BILIBILI, fid.to_owned()))
        .name(info.title)
        .description(info.intro.unwrap_or_default())
        .cover_url(info.cover.as_deref().and_then(cover_media_url))
        .track_count(u64::try_from(info.media_count).unwrap_or(0))
        .songs(songs)
        .build()
}

#[cfg(test)]
mod tests {
    use mineral_model::{AlbumId, ArtistId, MediaUrl, SongId, SourceKind};

    use super::{search_video_to_song, view_to_album};
    use crate::wire::de::from_value;
    use crate::wire::search::SearchVideoItem;
    use crate::wire::view::VideoInfo;

    /// playurl → PlayUrl:取 id 最大的音频轨、映射 Exhigh/AAC、**带上 Referer 取流头**
    /// (B站 baseUrl 播放必需,否则 403),bitrate 落 bandwidth。
    ///
    /// 回归:真实响应每项**同时**带 `baseUrl` + `base_url`(值同)。DTO 只认 `baseUrl`,
    /// 不给 `base_url` alias,否则 serde 报 `duplicate field baseUrl`、取流失败卡开头。
    #[test]
    fn playurl_maps_best_audio_with_referer() -> color_eyre::Result<()> {
        use super::playurl_to_play;
        use crate::wire::playurl::PlayUrlResult;

        let raw = serde_json::json!({
            "dash": { "audio": [
                { "id": 30216, "baseUrl": "https://cdn/64k.m4s", "base_url": "https://cdn/64k.m4s", "bandwidth": 64000, "codecs": "mp4a.40.2" },
                { "id": 30280, "baseUrl": "https://cdn/192k.m4s", "base_url": "https://cdn/192k.m4s", "bandwidth": 320000, "codecs": "mp4a.40.2" }
            ] }
        });
        let dto: PlayUrlResult = from_value(raw)?;
        let pu = playurl_to_play(SongId::new(SourceKind::BILIBILI, "BV1xx:1"), dto)
            .ok_or_else(|| color_eyre::eyre::eyre!("应产出 PlayUrl"))?;
        assert_eq!(
            pu.url,
            MediaUrl::remote("https://cdn/192k.m4s")?,
            "取 id 最大轨"
        );
        assert_eq!(pu.quality, mineral_model::BitRate::Exhigh);
        assert_eq!(pu.bitrate_bps, 320_000);
        assert_eq!(
            pu.stream_headers,
            vec![("Referer".to_owned(), "https://www.bilibili.com".to_owned())],
            "取流必须带 Referer,否则 baseUrl 403"
        );
        Ok(())
    }

    /// flac 轨存在时优先取无损(Lossless/Flac)。
    #[test]
    fn playurl_prefers_flac_when_present() -> color_eyre::Result<()> {
        use super::playurl_to_play;
        use crate::wire::playurl::PlayUrlResult;

        let raw = serde_json::json!({
            "dash": {
                "audio": [ { "id": 30280, "baseUrl": "https://cdn/192k.m4s", "bandwidth": 320000, "codecs": "mp4a.40.2" } ],
                "flac": { "audio": { "id": 30251, "baseUrl": "https://cdn/flac.m4s", "bandwidth": 900000, "codecs": "fLaC" } }
            }
        });
        let dto: PlayUrlResult = from_value(raw)?;
        let pu = playurl_to_play(SongId::new(SourceKind::BILIBILI, "BV1xx:1"), dto)
            .ok_or_else(|| color_eyre::eyre::eyre!("应产出 PlayUrl"))?;
        assert_eq!(pu.url, MediaUrl::remote("https://cdn/flac.m4s")?);
        assert_eq!(pu.quality, mineral_model::BitRate::Lossless);
        assert_eq!(pu.format, mineral_model::AudioFormat::Flac);
        Ok(())
    }

    /// 搜索项 → Song:strip `<em>`、`"3:45"` → 225000ms、`//x.jpg` → `https://x.jpg`、
    /// SongId = `{bvid}:1`、album 落 BV 专辑、artist 落 UP 主。
    #[test]
    fn search_item_maps_to_song() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "bvid": "BV1xx", "title": "【<em class=\"keyword\">初音</em>】曲名",
            "author": "UP主甲", "mid": 12345, "pic": "//x.jpg", "duration": "3:45"
        });
        let item: SearchVideoItem = from_value(raw)?;
        let song =
            search_video_to_song(item).ok_or_else(|| color_eyre::eyre::eyre!("应产出 Song"))?;
        assert_eq!(song.name, "【初音】曲名");
        assert_eq!(song.duration_ms, 225_000);
        assert_eq!(song.cover_url, MediaUrl::remote("https://x.jpg").ok());
        assert_eq!(song.id, SongId::new(SourceKind::BILIBILI, "BV1xx:1"));
        let album = song
            .album
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有专辑"))?;
        assert_eq!(album.id, AlbumId::new(SourceKind::BILIBILI, "BV1xx"));
        let artist = song
            .artists
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有艺术家"))?;
        assert_eq!(artist.id, ArtistId::new(SourceKind::BILIBILI, "12345"));
        assert_eq!(artist.name, "UP主甲");
        Ok(())
    }

    /// 缺 bvid → 无法定位,返回 None。
    #[test]
    fn search_item_without_bvid_is_none() -> color_eyre::Result<()> {
        let item: SearchVideoItem = from_value(serde_json::json!({ "title": "x" }))?;
        assert!(search_video_to_song(item).is_none());
        Ok(())
    }

    /// 多 P 视频 → Album:逐 P 成曲(SongId = `{bvid}:{page}`)、track_count = P 数、
    /// pubdate 秒 → 毫秒。附完整 Debug 快照。
    #[test]
    fn multi_page_video_maps_each_page_to_song() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "bvid": "BV1xx", "title": "多P合集", "cid": 1001,
            "pic": "//cover.jpg", "duration": 440, "pubdate": 1_600_000_000_i64,
            "desc": "合集简介",
            "owner": { "mid": 12345, "name": "UP主甲" },
            "pages": [
                { "cid": 1001, "page": 1, "part": "第一话", "duration": 240 },
                { "cid": 1002, "page": 2, "part": "第二话", "duration": 200 }
            ]
        });
        let info: VideoInfo = from_value(raw)?;
        let album = view_to_album(info);
        assert_eq!(album.id, AlbumId::new(SourceKind::BILIBILI, "BV1xx"));
        assert_eq!(album.track_count, 2);
        assert_eq!(album.songs.len(), 2);
        let s0 = album
            .songs
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有首曲"))?;
        let s1 = album
            .songs
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("应有次曲"))?;
        assert_eq!(s0.id, SongId::new(SourceKind::BILIBILI, "BV1xx:1"));
        assert_eq!(s1.id, SongId::new(SourceKind::BILIBILI, "BV1xx:2"));
        assert_eq!(s0.name, "第一话");
        assert_eq!(s0.duration_ms, 240_000);
        assert_eq!(album.publish_time_ms, 1_600_000_000_000);
        mineral_test::assert_snap_debug!(
            "多P视频详情 → Album(2P 逐 P 成曲 + owner + 封面补 https)",
            album
        );
        Ok(())
    }

    /// 单 P 视频(`pages` 缺失)→ 1 首曲目,SongId = `{bvid}:1`、曲名退回视频标题。
    #[test]
    fn single_page_without_pages_maps_to_one_song() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "bvid": "BV1yy", "title": "单P视频", "cid": 2001, "duration": 100,
            "owner": { "mid": 7, "name": "乙" }
        });
        let info: VideoInfo = from_value(raw)?;
        let album = view_to_album(info);
        assert_eq!(album.songs.len(), 1);
        let s = album
            .songs
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有单曲"))?;
        assert_eq!(s.id, SongId::new(SourceKind::BILIBILI, "BV1yy:1"));
        assert_eq!(s.name, "单P视频");
        assert_eq!(s.duration_ms, 100_000);
        Ok(())
    }

    /// 收藏夹条目 → Song:bvid→SongId `bvid:1`、upper→artist、duration 秒→ms、封面补 https。
    #[test]
    fn fav_media_maps_to_song() -> color_eyre::Result<()> {
        use super::fav_media_to_song;
        use crate::wire::fav::FavMedia;

        let raw = serde_json::json!({
            "bvid": "BV1zz", "title": "收藏的歌", "cover": "//i0.hdslb.com/c.jpg",
            "duration": 200, "upper": { "mid": 999, "name": "收藏UP" }
        });
        let media: FavMedia = from_value(raw)?;
        let song =
            fav_media_to_song(media).ok_or_else(|| color_eyre::eyre::eyre!("应产出 Song"))?;
        assert_eq!(song.id, SongId::new(SourceKind::BILIBILI, "BV1zz:1"));
        assert_eq!(song.duration_ms, 200_000);
        assert_eq!(
            song.cover_url,
            MediaUrl::remote("https://i0.hdslb.com/c.jpg").ok()
        );
        let artist = song
            .artists
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 artist"))?;
        assert_eq!(artist.id, ArtistId::new(SourceKind::BILIBILI, "999"));
        Ok(())
    }

    /// 收藏夹 folder → Playlist:id 入 BILIBILI namespace、track_count = media_count、
    /// **封面补 https 落 cover_url、intro 落 description**。
    ///
    /// 回归:曾用 `created/list-all` 端点(每项无 cover)拉收藏夹列表,列表封面全回退 hash;
    /// 换分页 `created/list`(每项带 cover / intro)后此处才有真实封面。
    #[test]
    fn fav_folder_maps_to_playlist() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use super::fav_folder_to_playlist;
        use crate::wire::fav::FavFolder;

        let raw = serde_json::json!({
            "id": 42, "title": "我的收藏", "media_count": 7,
            "cover": "//i0.hdslb.com/bfs/archive/x.jpg", "intro": "夹子简介"
        });
        let folder: FavFolder = from_value(raw)?;
        let pl = fav_folder_to_playlist(folder);
        assert_eq!(pl.id, PlaylistId::new(SourceKind::BILIBILI, "42"));
        assert_eq!(pl.name, "我的收藏");
        assert_eq!(pl.track_count, 7);
        assert_eq!(
            pl.cover_url,
            MediaUrl::remote("https://i0.hdslb.com/bfs/archive/x.jpg").ok(),
            "folder cover 协议相对应补 https 落 cover_url"
        );
        assert_eq!(pl.description, "夹子简介", "intro 落 description");
        Ok(())
    }
}

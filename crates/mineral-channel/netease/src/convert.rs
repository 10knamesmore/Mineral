//! 网易原生 DTO → `mineral_model` 类型的转换 helper。

use mineral_model::{
    Album, AlbumId, AlbumRef, Artist, ArtistId, ArtistRef, AudioFormat, BitRate, MediaUrl, PlayUrl,
    Playlist, PlaylistId, Song, SongId, SourceKind,
};

use crate::wire::artist::{ArtistAlbum, ArtistDetailResult};
use crate::wire::playlist::PlaylistInfo;
use crate::wire::search::{AlbumDetailResult, SearchAlbum, SearchArtist, SearchPlaylist};
use crate::wire::song::{AlbumSong, Artist as WireArtist, SongUrl};

/// 把网易 JSON 里的字符串字段(永远是 http(s) URL)转成 `MediaUrl::Remote`。
///
/// 解析失败或空字符串返回 `None`,让 `Option<MediaUrl>` 字段保持空。
pub fn parse_remote(s: &str) -> Option<MediaUrl> {
    if s.is_empty() {
        return None;
    }
    MediaUrl::remote(s).ok()
}

/// `Option<&str>` 版的便利包装。
pub fn parse_remote_opt(s: Option<&str>) -> Option<MediaUrl> {
    s.and_then(parse_remote)
}

/// album 的 artist 列表 → `Vec<ArtistRef>`:优先全列表 `artists`,为空时退回主 artist
/// `primary`(部分端点只给单个 `artist`)。
pub(crate) fn album_artist_refs(
    artists: Vec<WireArtist>,
    primary: Option<WireArtist>,
) -> Vec<ArtistRef> {
    let list = if artists.is_empty() {
        primary.into_iter().collect::<Vec<WireArtist>>()
    } else {
        artists
    };
    list.into_iter()
        .map(|a| ArtistRef {
            id: ArtistId::new(SourceKind::NETEASE, a.id.to_string()),
            name: a.name,
        })
        .collect()
}

/// 专辑元信息 DTO + 已解析曲目 → 统一 [`Album`]。
///
/// 搜索与详情共用:搜索结果只有元信息(`songs` 传空),详情端点元信息 + 曲目都全。
pub(crate) fn album_dto_to_model(a: SearchAlbum, songs: Vec<Song>) -> Album {
    Album::builder()
        .id(AlbumId::new(SourceKind::NETEASE, a.id.to_string()))
        .name(a.name)
        .artists(album_artist_refs(a.artists, a.artist))
        .description(a.description)
        .company(a.company)
        .publish_time_ms(a.publish_time)
        .track_count(Some(a.size))
        .cover_url(a.pic_url.as_deref().and_then(parse_remote))
        .songs(songs)
        .build()
}

/// 专辑详情响应(元信息 + 曲目)→ 统一 [`Album`]。
pub(crate) fn album_detail_to_model(r: AlbumDetailResult) -> Album {
    let songs = r
        .songs
        .into_iter()
        .map(album_song_to_model)
        .collect::<Vec<Song>>();
    album_dto_to_model(r.album, songs)
}

/// 歌单元信息 DTO + 已解析曲目 → 统一 [`Playlist`]。
///
/// 详情与用户列表共用:详情端点元信息 + 曲目都有(`songs` 传全拉/缓存重建的曲目);
/// 用户列表项只有元信息(`songs` 传空)。
pub(crate) fn playlist_info_to_model(info: &PlaylistInfo, songs: Vec<Song>) -> Playlist {
    Playlist::builder()
        .id(PlaylistId::new(SourceKind::NETEASE, info.id.to_string()))
        .name(info.name.clone())
        .description(info.description.clone())
        .cover_url(info.cover_img_url.as_deref().and_then(parse_remote))
        .track_count(info.track_count)
        .play_count(info.play_count)
        .subscriber_count(info.subscribed_count)
        .songs(songs)
        .build()
}

/// 专辑/歌单/歌手详情里的 [`AlbumSong`](ar/al/dt 字段风格)→ 统一 [`Song`]。
pub(crate) fn album_song_to_model(s: AlbumSong) -> Song {
    Song::builder()
        .id(SongId::new(SourceKind::NETEASE, s.id.to_string()))
        .name(s.name)
        .translation(s.tns.into_iter().next())
        .artists(
            s.ar.into_iter()
                .map(|a| ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, a.id.to_string()),
                    name: a.name,
                })
                .collect(),
        )
        .album(Some(AlbumRef {
            id: AlbumId::new(SourceKind::NETEASE, s.al.id.to_string()),
            name: s.al.name,
        }))
        // 接口用 dt=0 表示时长未知,在 channel 边界转成 None,0 哨兵不进模型。
        .duration_ms((s.dt > 0).then_some(s.dt))
        .cover_url(s.al.pic_url.as_deref().and_then(parse_remote))
        .unavailable(s.privilege.as_ref().is_some_and(|p| p.st < 0))
        .build()
}

/// 搜索结果歌手 DTO → 统一 [`Artist`]。
///
/// 搜索只给元信息 + 粉丝数(`fansSize`),不含简介/热门曲——简介与曲目按需走 `artist_detail`。
pub(crate) fn search_artist_to_model(a: SearchArtist) -> Artist {
    Artist::builder()
        .id(ArtistId::new(SourceKind::NETEASE, a.id.to_string()))
        .name(a.name)
        .follower_count(a.fans_size)
        .avatar_url(parse_remote_opt(a.pic_url.as_deref()))
        .build()
}

/// 搜索结果歌单 DTO → 统一 [`Playlist`](只有元信息,曲目按需走 `playlist_detail`)。
pub(crate) fn search_playlist_to_model(p: SearchPlaylist) -> Playlist {
    Playlist::builder()
        .id(PlaylistId::new(SourceKind::NETEASE, p.id.to_string()))
        .name(p.name)
        .description(p.description.unwrap_or_default())
        .cover_url(p.cover_img_url.as_deref().and_then(parse_remote))
        .track_count(p.track_count)
        .play_count(p.play_count)
        .subscriber_count(p.book_count)
        .build()
}

/// 歌手详情响应 + 粉丝数 → 统一 [`Artist`]。
///
/// `fans` 来自 `/api/artist/follow/count/get`——详情端点顶层不带粉丝数,由 channel 并发补打
/// 后传入(补打失败为 `None`)。`albumSize`/`musicSize` 是详情端点顶层独家给的名下专辑/歌曲计数。
pub(crate) fn artist_detail_to_model(r: ArtistDetailResult, fans: Option<u64>) -> Artist {
    Artist::builder()
        .id(ArtistId::new(SourceKind::NETEASE, r.artist.id.to_string()))
        .name(r.artist.name)
        .description(r.artist.brief_desc)
        .follower_count(fans)
        .album_count(r.artist.album_size)
        .song_count(r.artist.music_size)
        .avatar_url(parse_remote_opt(r.artist.pic_url.as_deref()))
        .songs(r.hot_songs.into_iter().map(album_song_to_model).collect())
        .build()
}

/// 歌手专辑列表项 → 统一 [`Album`](曲目留空,按需走 `album_detail`)。
pub(crate) fn artist_album_to_model(a: ArtistAlbum) -> Album {
    Album::builder()
        .id(AlbumId::new(SourceKind::NETEASE, a.id.to_string()))
        .name(a.name)
        .artists(album_artist_refs(a.artists, a.artist))
        .company(a.company)
        .publish_time_ms(a.publish_time)
        .track_count(Some(a.size))
        .cover_url(parse_remote_opt(a.pic_url.as_deref()))
        .build()
}

/// 播放 URL 端点 DTO 列表 → [`PlayUrl`] 列表,丢弃不可播的项(判据见 [`song_url_to_play`])。
///
/// `quality` 是请求时的目标音质,原样回填 [`PlayUrl`] 的 `quality`(响应不含等级回执)。
pub(crate) fn play_urls(dtos: Vec<SongUrl>, quality: BitRate) -> Vec<PlayUrl> {
    dtos.into_iter()
        .filter_map(|d| song_url_to_play(d, quality))
        .collect()
}

/// 整批条目是否都**显式**报了「本源无资源」(单曲级 `code` 非 200)。
///
/// 全灰时 legacy 端点同样不会有资源,调用方据此省掉降级那一跳、直接返回空
/// (task 层继而发 `SongUrlFailed`,unplayable 拦截口接手)。只认显式 code——
/// code 缺失或试听片段不算,那些情形 legacy 仍可能给出完整流。
pub(crate) fn all_explicitly_unavailable(dtos: &[SongUrl]) -> bool {
    !dtos.is_empty() && dtos.iter().all(|d| d.code.is_some_and(|c| c != 200))
}

/// 单条播放 URL DTO → [`PlayUrl`];不可播返回 `None`。
///
/// 不可播三口:单曲级 `code` 非 200(实测无版权曲为 404,url 同时为 null)、
/// `freeTrialInfo` 非空(url 只是试听片段——接受它会把片段当整曲播放并入缓存)、
/// url 缺失。
fn song_url_to_play(d: SongUrl, quality: BitRate) -> Option<PlayUrl> {
    if d.code.is_some_and(|c| c != 200) || d.free_trial_info.is_some() {
        return None;
    }
    let url = MediaUrl::remote(&d.url?).ok()?;
    Some(PlayUrl {
        song_id: SongId::new(SourceKind::NETEASE, d.id.to_string()),
        url,
        // wire 层 br / size 缺字段经 serde default 落 0,在此边界转 None,哨兵不进模型。
        bitrate_bps: (d.br > 0).then_some(d.br),
        quality,
        size: (d.size > 0).then_some(d.size),
        format: d.format.filter(|s| !s.is_empty()).map(AudioFormat::from),
        // 网易云播放接口的响应不含位深字段(实测 /song/url/v1 无 bitDepth),恒 None。
        bit_depth: None,
        // 网易云音频 CDN 直链自足,不需附加取流头。
        stream_headers: Vec::new(),
        // 网易云是整块直链(MP3/FLAC),随机访问廉价,正常 seekable 打开。
        layout: mineral_model::StreamLayout::Contiguous,
        substituted: false,
    })
}

#[cfg(test)]
mod tests {
    use mineral_model::{AlbumId, SourceKind};

    use super::album_detail_to_model;
    use crate::wire::de::from_value;
    use crate::wire::search::AlbumDetailResult;

    /// 灰歌条目(code=404 + url null,真实响应形态)→ 滤掉;整批全灰 →
    /// `all_explicitly_unavailable` 为 true(channel 据此不降级 legacy)。
    #[test]
    fn grey_entry_is_rejected_and_detected() -> color_eyre::Result<()> {
        use mineral_model::BitRate;

        use super::{all_explicitly_unavailable, play_urls};
        use crate::wire::song::SongUrl;

        // 2026-07-04 实测 /song/enhance/player/url/v1 对无版权曲(id 186016)的形态。
        let grey: SongUrl = from_value(serde_json::json!({
            "id": 186016, "code": 404, "url": null, "br": 0, "size": 0, "type": null,
            "freeTrialInfo": null
        }))?;
        assert!(
            all_explicitly_unavailable(std::slice::from_ref(&grey)),
            "整批显式非 200 应判定为本源无资源"
        );
        assert!(
            play_urls(vec![grey], BitRate::Exhigh).is_empty(),
            "灰歌条目不得产出 PlayUrl"
        );
        assert!(
            !all_explicitly_unavailable(&[]),
            "空批不算全灰(v1 异常形态,交给 legacy 降级)"
        );
        Ok(())
    }

    /// 试听片段(code=200 但 freeTrialInfo 非空)→ 不算可播,滤掉;且**不**算显式无资源
    /// (legacy 可能给完整流,降级仍要走)。
    #[test]
    fn trial_fragment_is_not_playable() -> color_eyre::Result<()> {
        use mineral_model::BitRate;

        use super::{all_explicitly_unavailable, play_urls};
        use crate::wire::song::SongUrl;

        let trial: SongUrl = from_value(serde_json::json!({
            "id": 33894312, "code": 200, "url": "https://m7.music.126.net/trial.mp3",
            "br": 320_000, "size": 1_048_576, "type": "mp3",
            "freeTrialInfo": { "start": 45, "end": 75 }
        }))?;
        assert!(
            !all_explicitly_unavailable(std::slice::from_ref(&trial)),
            "试听不算显式无资源,legacy 降级仍要走"
        );
        assert!(
            play_urls(vec![trial], BitRate::Exhigh).is_empty(),
            "试听片段不得当完整可播流放行"
        );
        Ok(())
    }

    /// 正常条目(code=200 + 完整 url)照常产出;混批(灰 + 正常)不触发全灰判定。
    #[test]
    fn playable_entry_survives_mixed_batch() -> color_eyre::Result<()> {
        use mineral_model::BitRate;

        use super::{all_explicitly_unavailable, play_urls};
        use crate::wire::song::SongUrl;

        let batch: Vec<SongUrl> = vec![
            from_value(serde_json::json!({
                "id": 186016, "code": 404, "url": null
            }))?,
            from_value(serde_json::json!({
                "id": 1_862_188_922, "code": 200,
                "url": "https://m7.music.126.net/full.mp3", "br": 320_000,
                "size": 8_388_608, "type": "mp3"
            }))?,
        ];
        assert!(!all_explicitly_unavailable(&batch), "混批不算全灰");
        let urls = play_urls(batch, BitRate::Exhigh);
        assert_eq!(urls.len(), 1, "只有可播条目产出");
        assert_eq!(urls.first().map(|u| u.song_id.as_str()), Some("1862188922"));
        Ok(())
    }

    /// 权限块 `st < 0`(实测下架灰歌 -200)→ `Song.unavailable`;
    /// `st = 0` / 权限块缺失(旧端点形态)→ 可播。
    #[test]
    fn privilege_st_negative_marks_unavailable() -> color_eyre::Result<()> {
        use super::album_song_to_model;
        use crate::wire::song::AlbumSong;

        // cloudsearch 形态:privilege 内联在歌曲对象上。
        let grey: AlbumSong = from_value(serde_json::json!({
            "id": 186_016, "name": "晴天",
            "ar": [{ "id": 6452, "name": "周杰伦" }],
            "al": { "id": 18896, "name": "葉惠美" },
            "dt": 269_000,
            "privilege": { "id": 186_016, "st": -200, "pl": 0, "dl": 0 }
        }))?;
        assert!(album_song_to_model(grey).unavailable, "st=-200 应判不可播");

        let normal: AlbumSong = from_value(serde_json::json!({
            "id": 1, "name": "ok", "al": { "id": 2, "name": "a" },
            "privilege": { "id": 1, "st": 0 }
        }))?;
        assert!(!album_song_to_model(normal).unavailable, "st=0 应可播");

        let missing: AlbumSong = from_value(serde_json::json!({
            "id": 3, "name": "no-priv", "al": { "id": 4, "name": "b" }
        }))?;
        assert!(
            !album_song_to_model(missing).unavailable,
            "权限块缺失不得误判不可播"
        );
        Ok(())
    }

    /// 专辑详情(顶层元信息 + 曲目)→ model:简介 / track_count / 曲目 / 封面 / id 都到位。
    /// 锁住"详情端点独家给的 description 不再被丢"这一重构要点。
    #[test]
    fn album_detail_maps_meta_and_songs() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "album": {
                "id": 3_314_467, "name": "Chinese Football",
                "description": "成军四年的首张全长专辑。", "company": null,
                "publishTime": 1_443_196_800_000_i64, "size": 13,
                "picUrl": "https://p3.music.126.net/x.jpg",
                "artists": [{ "id": 1_081_839, "name": "Chinese Football" }]
            },
            "songs": [
                { "id": 1, "name": "电动少女",
                  "ar": [{ "id": 1_081_839, "name": "Chinese Football" }],
                  "al": { "id": 3_314_467, "name": "Chinese Football" }, "dt": 310_000 }
            ]
        });
        let dto: AlbumDetailResult = from_value(raw)?;
        let album = album_detail_to_model(dto);
        assert_eq!(album.name, "Chinese Football");
        assert_eq!(album.description, "成军四年的首张全长专辑。");
        assert_eq!(album.track_count, Some(13));
        assert_eq!(album.songs.len(), 1);
        assert!(album.cover_url.is_some());
        assert_eq!(album.id, AlbumId::new(SourceKind::NETEASE, "3314467"));
        Ok(())
    }

    /// 歌单详情(顶层元信息 + 曲目)→ model:简介 / 计数 / play_count / 曲目都到位。
    #[test]
    fn playlist_detail_maps_meta_and_songs() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use super::{album_song_to_model, playlist_info_to_model};
        use crate::wire::playlist::PlaylistDetailResult;

        let raw = serde_json::json!({
            "playlist": {
                "id": 12345, "name": "City Pop", "description": "昭和旋律",
                "coverImgUrl": "https://p.music.126.net/c.jpg",
                "trackCount": 147, "playCount": 8_941_862, "subscribedCount": 110_577,
                "trackUpdateTime": 1_700_000_000_000_i64,
                "trackIds": [{ "id": 1 }, { "id": 2 }],
                "tracks": [
                    { "id": 1, "name": "Plastic Love",
                      "ar": [{ "id": 9, "name": "竹内まりや" }],
                      "al": { "id": 3, "name": "VARIETY" }, "dt": 290_000 }
                ]
            }
        });
        let dto: PlaylistDetailResult = from_value(raw)?;
        let mut info = dto.playlist;
        let songs = std::mem::take(&mut info.tracks)
            .into_iter()
            .map(album_song_to_model)
            .collect::<Vec<_>>();
        let pl = playlist_info_to_model(&info, songs);
        assert_eq!(pl.name, "City Pop");
        assert_eq!(pl.description, "昭和旋律");
        assert_eq!(pl.track_count, 147);
        assert_eq!(pl.play_count, Some(8_941_862));
        assert_eq!(pl.subscriber_count, Some(110_577));
        assert_eq!(pl.songs.len(), 1);
        assert_eq!(pl.id, PlaylistId::new(SourceKind::NETEASE, "12345"));
        Ok(())
    }

    /// 歌手详情响应 → model:id 入 NETEASE namespace、briefDesc → description、
    /// albumSize/musicSize → 计数、hotSongs → songs 全量映射、fans 来自聚合参数。
    #[test]
    fn detail_maps_to_model_artist() -> color_eyre::Result<()> {
        use super::artist_detail_to_model;
        use crate::wire::artist::ArtistDetailResult;

        // 详情端点不带粉丝数(顶层无 fansCount),粉丝数由 follow/count 端点单独取、聚合时传入。
        let raw = serde_json::json!({
            "artist": { "id": 11127, "name": "Beyond", "briefDesc": "香港摇滚乐队",
                        "picUrl": "https://p1.music.126.net/x.jpg",
                        "albumSize": 146, "musicSize": 2570 },
            "hotSongs": [
                { "id": 1, "name": "海阔天空", "ar": [{ "id": 11127, "name": "Beyond" }],
                  "al": { "id": 9, "name": "乐与怒" }, "dt": 323_000 }
            ]
        });
        let dto: ArtistDetailResult = from_value(raw)?;
        let model = artist_detail_to_model(dto, /*fans*/ Some(8_900_000));
        assert_eq!(model.album_count, Some(146));
        assert_eq!(model.song_count, Some(2570));
        assert_eq!(model.follower_count, Some(8_900_000));
        mineral_test::assert_snap_debug!("歌手详情映射成统一 Artist(Beyond + 1 热门曲)", model);
        Ok(())
    }

    /// 歌手专辑列表项 → model:无主艺术家时 artists 为空,曲目恒空。
    #[test]
    fn album_item_without_artist_maps_to_empty_artists() -> color_eyre::Result<()> {
        use super::artist_album_to_model;
        use crate::wire::artist::ArtistAlbum;

        let raw =
            serde_json::json!({ "id": 8, "name": "继续革命", "publishTime": 715_000_000_000_i64 });
        let dto: ArtistAlbum = from_value(raw)?;
        let model = artist_album_to_model(dto);
        assert!(model.artists.is_empty());
        assert!(model.songs.is_empty());
        assert_eq!(model.publish_time_ms, 715_000_000_000);
        Ok(())
    }

    /// 建单响应(`playlist` 对象,复用 `PlaylistInfo` 形态)→ model:
    /// NETEASE namespace、null 描述 → 空串、无曲目、play/subscriber 计数留 None。
    #[test]
    fn create_response_maps_to_playlist() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use super::playlist_info_to_model;
        use crate::wire::playlist::CreatePlaylistResult;

        let raw = serde_json::json!({
            "code": 200, "id": 987_654,
            "playlist": { "id": 987_654, "name": "开车歌单", "trackCount": 0,
                          "coverImgUrl": "https://p1.music.126.net/c.jpg", "description": null }
        });
        let dto: CreatePlaylistResult = from_value(raw)?;
        let pl = playlist_info_to_model(&dto.playlist, Vec::new());
        assert_eq!(pl.id, PlaylistId::new(SourceKind::NETEASE, "987654"));
        assert_eq!(pl.name, "开车歌单");
        assert_eq!(pl.description, "");
        assert_eq!(pl.track_count, 0);
        assert!(pl.cover_url.is_some());
        assert!(pl.play_count.is_none());
        assert!(pl.subscriber_count.is_none());
        Ok(())
    }
}

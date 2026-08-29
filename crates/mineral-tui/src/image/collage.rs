//! mineral 源歌单(跨源聚合,无自带封面)的真封面拼贴。
//!
//! 取歌单内前 [`MAX_TILES`] 首有封面的歌当成员,成员图经图片引擎按需解码,
//! 已就绪的拼成一张方图,以合成键(`mineral://collage/<指纹>`)塞进图片引擎的解码图缓存——
//! 自那一步起与普通封面同管线(kitty 编码 / halfblock 降级 / LRU)。
//!
//! **渐进式重拼**：成员 decode 失败不阻塞已就绪图片；有几张拼几张，就绪数超过上次记录
//! 才重拼覆盖同 key，每歌单至多拼
//! [`MAX_TILES`] 次。成员集(前 N 首)变化即换新指纹键,旧图交给 LRU 自然逐出。

use std::sync::Arc;

use image::DynamicImage;
use image::imageops::FilterType;
use mineral_model::{MediaUrl, Playlist, SourceKind};

use crate::runtime::state::AppState;
use crate::runtime::view_model::PlaylistEntryView;

/// 拼贴最多取的成员封面数(2×2)。
const MAX_TILES: usize = 4;

/// 每 tick 调一次:给缺封面的 mineral 源歌单请求成员封面、就绪数增加时(重)拼贴入缓存。
///
/// 稳态开销:每歌单一次成员扫描(带封面前 [`MAX_TILES`] 首,早停)+ 若干 hash 探测;
/// 拼贴本身每歌单一生至多 [`MAX_TILES`] 次,同步完成成员裁剪与合成。
pub(crate) fn tick(state: &mut AppState) {
    let mut requests = Vec::<(SourceKind, MediaUrl)>::new();
    let mut composed = Vec::<(MediaUrl, usize, DynamicImage)>::new();
    for p in &state.library.playlists {
        if p.data.cover_url.is_some() || p.data.source() != SourceKind::MINERAL {
            continue;
        }
        let Some(tracks) = state.library.tracks.get(&p.data.id) else {
            continue;
        };
        let members = member_covers(tracks);
        let Some(key) = collage_key(&p.data, &members) else {
            continue;
        };
        for (source, url) in &members {
            if !state.images.cache.contains_key(url) && !state.images.pending.contains(url) {
                requests.push((*source, (*url).clone()));
            }
        }
        // 先用 contains_key 数就绪(不 touch LRU):已按当前就绪数拼过就别再逐张 get
        // 续命成员图——拼完的成员图让 LRU 自然淘汰。
        let ready_count = members
            .iter()
            .filter(|(_, url)| state.images.cache.contains_key(url))
            .count();
        let recorded = state.images.collage_ready.get(&key).copied().unwrap_or(0);
        if ready_count == 0 || (state.images.cache.contains_key(&key) && recorded >= ready_count) {
            continue;
        }
        let ready = members
            .iter()
            .filter_map(|(_, url)| state.images.cache.get(url).cloned())
            .collect::<Vec<Arc<DynamicImage>>>();
        let Some(image) = compose(&ready) else {
            continue;
        };
        composed.push((key, ready.len(), image));
    }
    state.images.load(requests);
    for (key, tiles, image) in composed {
        mineral_log::debug!(target: "cover", key = %key, tiles, "歌单拼贴合成入缓存");
        state.images.insert_synthesized(&key, Arc::new(image));
        state.images.collage_ready.insert(key, tiles);
    }
}

/// 渲染路径:歌单的有效封面 URL。
///
/// 自带 `cover_url` 直接给;mineral 源无封面歌单在拼贴已就绪(合成键命中缓存)时给
/// 合成键；其余返回 `None`，调用方留空，拼贴到货后自然切换。
///
/// # Params:
///   - `playlist`: 目标歌单
///
/// # Return:
///   可交给 [`crate::image::ImageContent::Display`] 的封面 URL。
pub(crate) fn effective_cover_url(state: &AppState, playlist: &Playlist) -> Option<MediaUrl> {
    if let Some(url) = &playlist.cover_url {
        return Some(url.clone());
    }
    if playlist.source() != SourceKind::MINERAL {
        return None;
    }
    let tracks = state.library.tracks.get(&playlist.id)?;
    let key = collage_key(playlist, &member_covers(tracks))?;
    state.images.cache.contains_key(&key).then_some(key)
}

/// 歌单内前 [`MAX_TILES`] 首**有封面**的歌的 `(来源, 封面 URL)`(按歌单顺序,早停)。
fn member_covers(tracks: &[PlaylistEntryView]) -> Vec<(SourceKind, &MediaUrl)> {
    tracks
        .iter()
        .filter_map(|entry| {
            entry
                .data
                .song
                .cover_url
                .as_ref()
                .map(|url| (entry.data.song.id.namespace(), url))
        })
        .take(MAX_TILES)
        .collect()
}

/// 拼贴合成键:`mineral://collage/<FNV-1a 64 指纹>`,指纹吃歌单 qualified id + 成员
/// 封面 URL 串(顺序敏感)。成员集变化 → 指纹变 → 新键,旧键图由 LRU 自然逐出。
///
/// # Params:
///   - `playlist`: 所属歌单
///   - `members`: [`member_covers`] 产物
///
/// # Return:
///   合成键;成员为空返回 `None`(没图可拼,键无意义)。
fn collage_key(playlist: &Playlist, members: &[(SourceKind, &MediaUrl)]) -> Option<MediaUrl> {
    if members.is_empty() {
        return None;
    }
    let mut fp = fnv64(FNV_OFFSET, playlist.id.qualified().as_bytes());
    for (_, url) in members {
        fp = fnv64(fp, url.to_string().as_bytes());
    }
    MediaUrl::remote(&format!("mineral://collage/{fp:016x}")).ok()
}

/// FNV-1a 64 位偏移基。
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// 把 `bytes` 揉进 FNV-1a 64 位滚动值。
fn fnv64(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// 把 1..=[`MAX_TILES`] 张成员图拼成方图。
///
/// 布局:1 张全铺;2 张左右对分;3 张左列整高 + 右列上下两块;4 张(及以上取前 4)
/// 2×2 四象限(左上 → 右上 → 左下 → 右下)。输出边长取成员最小短边，避免放大任一
/// 来源图；每块 `resize_to_fill` 居中裁剪。
///
/// # Params:
///   - `images`: 已就绪的成员图(歌单顺序)
/// # Return:
///   拼好的方图;`images` 为空返回 `None`。
fn compose(images: &[Arc<DynamicImage>]) -> Option<DynamicImage> {
    let size = images
        .iter()
        .map(|image| image.width().min(image.height()))
        .min()?
        .max(2);
    let half = size / 2;
    let rest = size - half;
    let tile = |img: &DynamicImage, w: u32, h: u32| {
        img.resize_to_fill(w, h, FilterType::Triangle).to_rgb8()
    };
    let mut canvas = image::RgbImage::new(size, size);
    let mut put = |img: &DynamicImage, x: u32, y: u32, w: u32, h: u32| {
        image::imageops::replace(&mut canvas, &tile(img, w, h), i64::from(x), i64::from(y));
    };
    match images {
        [] => return None,
        [a] => put(a, 0, 0, size, size),
        [a, b] => {
            put(a, 0, 0, half, size);
            put(b, half, 0, rest, size);
        }
        [a, b, c] => {
            put(a, 0, 0, half, size);
            put(b, half, 0, rest, half);
            put(c, half, half, rest, rest);
        }
        [a, b, c, d, ..] => {
            put(a, 0, 0, half, half);
            put(b, half, 0, rest, half);
            put(c, 0, half, half, rest);
            put(d, half, half, rest, rest);
        }
    }
    Some(DynamicImage::ImageRgb8(canvas))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use color_eyre::eyre::eyre;
    use image::{DynamicImage, Rgb, RgbImage};
    use mineral_model::{MediaUrl, PlaylistId, Song, SongId, SourceKind};

    use super::{collage_key, compose, effective_cover_url, member_covers, tick};
    use crate::runtime::state::AppState;
    use crate::test_support::entry_views;

    /// 造一张纯色方图。
    fn solid(rgb: [u8; 3], side: u32) -> Arc<DynamicImage> {
        let mut img = RgbImage::new(side, side);
        for p in img.pixels_mut() {
            *p = Rgb(rgb);
        }
        Arc::new(DynamicImage::ImageRgb8(img))
    }

    /// 断言合成图 `(x, y)` 像素为 `rgb`。
    fn assert_px(img: &DynamicImage, x: u32, y: u32, rgb: [u8; 3]) -> color_eyre::Result<()> {
        let px = img
            .to_rgb8()
            .get_pixel_checked(x, y)
            .copied()
            .ok_or_else(|| eyre!("像素 ({x},{y}) 越界"))?;
        assert_eq!(px, Rgb(rgb), "像素 ({x},{y}) 颜色");
        Ok(())
    }

    /// 四张纯色图 → 2×2 四象限(左上红、右上绿、左下蓝、右下黄),纯色无缝可精确断言。
    #[test]
    fn compose_four_fills_quadrants() -> color_eyre::Result<()> {
        let imgs = vec![
            solid([200, 0, 0], 8),
            solid([0, 200, 0], 8),
            solid([0, 0, 200], 8),
            solid([200, 200, 0], 8),
        ];
        let out = compose(&imgs).ok_or_else(|| eyre!("应拼出图"))?;
        assert_eq!((out.width(), out.height()), (8, 8), "输出使用成员最小短边");
        assert_px(&out, 2, 2, [200, 0, 0])?;
        assert_px(&out, 6, 2, [0, 200, 0])?;
        assert_px(&out, 2, 6, [0, 0, 200])?;
        assert_px(&out, 6, 6, [200, 200, 0])?;
        Ok(())
    }

    /// 一张全铺;两张左右对分;三张左列整高 + 右列上下。
    #[test]
    fn compose_degraded_layouts() -> color_eyre::Result<()> {
        let one = compose(&[solid([200, 0, 0], 8)]).ok_or_else(|| eyre!("1 张应拼出"))?;
        assert_px(&one, 1, 1, [200, 0, 0])?;
        assert_px(&one, 6, 6, [200, 0, 0])?;

        let two = compose(&[solid([200, 0, 0], 8), solid([0, 200, 0], 8)])
            .ok_or_else(|| eyre!("2 张应拼出"))?;
        assert_px(&two, 2, 4, [200, 0, 0])?;
        assert_px(&two, 6, 4, [0, 200, 0])?;

        let three = compose(&[
            solid([200, 0, 0], 8),
            solid([0, 200, 0], 8),
            solid([0, 0, 200], 8),
        ])
        .ok_or_else(|| eyre!("3 张应拼出"))?;
        assert_px(&three, 2, 2, [200, 0, 0])?;
        assert_px(&three, 2, 6, [200, 0, 0])?;
        assert_px(&three, 6, 2, [0, 200, 0])?;
        assert_px(&three, 6, 6, [0, 0, 200])?;
        Ok(())
    }

    /// 空成员拼不出图。
    #[test]
    fn compose_empty_is_none() {
        assert!(compose(&[]).is_none(), "空成员应返回 None");
    }

    /// 造一首带封面的歌(id/封面按序号区分,归属 `source`)。
    fn song_with_cover(source: SourceKind, i: usize) -> color_eyre::Result<Song> {
        Ok(Song::builder()
            .id(SongId::new(source, format!("s{i}")))
            .name(format!("song {i}"))
            .duration_ms(Some(1000))
            .cover_url(Some(MediaUrl::remote(&format!("https://cover/{i}.jpg"))?))
            .build())
    }

    /// 成员选取:跳过无封面的歌、按序取前 4 首、超出截断。
    #[test]
    fn member_covers_picks_first_four_with_cover() -> color_eyre::Result<()> {
        let no_cover = Song::builder()
            .id(SongId::new(SourceKind::NETEASE, "s0"))
            .name("no cover".to_owned())
            .duration_ms(Some(1000))
            .build();
        let mut songs = vec![no_cover];
        for i in 1..=6 {
            songs.push(song_with_cover(SourceKind::NETEASE, i)?);
        }
        let tracks = entry_views(songs);
        let members = member_covers(&tracks);
        assert_eq!(members.len(), 4, "取前 4 首有封面的");
        let first = members.first().map(|(_, u)| u.to_string());
        assert_eq!(
            first.as_deref(),
            Some("https://cover/1.jpg"),
            "无封面首曲被跳过"
        );
        let last = members.last().map(|(_, u)| u.to_string());
        assert_eq!(
            last.as_deref(),
            Some("https://cover/4.jpg"),
            "第 5 首起截断"
        );
        Ok(())
    }

    /// 造一个 mineral 聚合歌单与 `n` 首带封面曲目的 state。
    fn mineral_state(n: usize) -> color_eyre::Result<AppState> {
        let mut s = AppState::test_default()?;
        let pid = PlaylistId::new(SourceKind::MINERAL, "favorites");
        s.library.playlists = vec![crate::test_support::playlist_view(
            "favorites",
            "Favorites",
            SourceKind::MINERAL,
            u64::try_from(n).unwrap_or(0),
        )];
        let songs = (0..n)
            .map(|i| song_with_cover(SourceKind::NETEASE, i))
            .collect::<color_eyre::Result<Vec<Song>>>()?;
        let tracks = entry_views(songs);
        s.library.tracks.insert(pid, tracks);
        Ok(s)
    }

    /// 第 `i` 首歌的封面 URL(与 [`song_with_cover`] 对应)。
    fn cover_url(i: usize) -> color_eyre::Result<MediaUrl> {
        Ok(MediaUrl::remote(&format!("https://cover/{i}.jpg"))?)
    }

    /// 成员图未就绪:tick 只把成员封面标 pending(请求已投),不合成。
    #[test]
    fn tick_requests_member_covers() -> color_eyre::Result<()> {
        let mut s = mineral_state(4)?;
        tick(&mut s);
        for i in 0..4 {
            assert!(
                s.images.pending.contains(&cover_url(i)?),
                "成员 {i} 封面应已标 pending"
            );
        }
        assert!(s.images.collage_ready.is_empty(), "无就绪成员不应合成");
        assert!(
            effective_cover_url(
                &s,
                &s.library
                    .playlists
                    .first()
                    .ok_or_else(|| eyre!("歌单在"))?
                    .data
            )
            .is_none(),
            "未合成时渲染侧应保持空白"
        );
        Ok(())
    }

    /// 渐进式重拼:2 张就绪先拼 2,其余到货后重拼成 4;渲染侧自合成起给合成键。
    #[test]
    fn tick_composes_progressively() -> color_eyre::Result<()> {
        let mut s = mineral_state(4)?;
        for i in 0..2 {
            s.images.cache.insert(&cover_url(i)?, solid([200, 0, 0], 8));
        }
        tick(&mut s);
        let playlist = s
            .library
            .playlists
            .first()
            .ok_or_else(|| eyre!("歌单在"))?
            .data
            .clone();
        let key = effective_cover_url(&s, &playlist).ok_or_else(|| eyre!("2 张就绪应已合成"))?;
        assert_eq!(
            s.images.collage_ready.get(&key).copied(),
            Some(2),
            "记录就绪数 2"
        );

        for i in 2..4 {
            s.images.cache.insert(&cover_url(i)?, solid([0, 200, 0], 8));
        }
        tick(&mut s);
        assert_eq!(
            s.images.collage_ready.get(&key).copied(),
            Some(4),
            "全员到货应重拼成 4"
        );

        // 就绪数没变,再 tick 不重拼(记录值不动、无新键)。
        tick(&mut s);
        assert_eq!(s.images.collage_ready.len(), 1, "稳态不再新增合成");
        Ok(())
    }

    /// 合成键确定性 + 成员敏感:同输入同键,成员变则键变;键是 Remote(mineral scheme)。
    #[test]
    fn collage_key_is_deterministic_and_member_sensitive() -> color_eyre::Result<()> {
        let s = mineral_state(4)?;
        let playlist = &s
            .library
            .playlists
            .first()
            .ok_or_else(|| eyre!("歌单在"))?
            .data;
        let tracks = s
            .library
            .tracks
            .get(&playlist.id)
            .ok_or_else(|| eyre!("曲目在"))?;
        let k1 = collage_key(playlist, &member_covers(tracks)).ok_or_else(|| eyre!("应有键"))?;
        let k2 = collage_key(playlist, &member_covers(tracks)).ok_or_else(|| eyre!("应有键"))?;
        assert_eq!(k1, k2, "同输入同键");
        assert!(k1.is_remote(), "合成键应为 Remote 变体");

        let fewer = tracks.get(..2).ok_or_else(|| eyre!("切片在"))?;
        let k3 = collage_key(playlist, &member_covers(fewer)).ok_or_else(|| eyre!("应有键"))?;
        assert_ne!(k1, k3, "成员集变化应换键");
        Ok(())
    }
}

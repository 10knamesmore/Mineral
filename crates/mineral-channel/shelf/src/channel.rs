//! shelf source 的 channel:读侧全部查本地索引(persist 的 `ShelfStore`)。
//!
//! 播放路由的强制接缝——`shelf:uuid` 的歌靠本 channel 被 `channel_for(SHELF)` 找到,
//! 才能播放 / 下载 / 进收藏聚合。取数极便宜:就是查本地 sqlite 索引。

use std::path::PathBuf;

use async_trait::async_trait;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, Error, MusicChannel, Page, Result, SearchHits,
};
use mineral_model::{
    AudioFormat, BitRate, MediaUrl, PlayUrl, Playlist, PlaylistId, SearchKind, Song, SongId,
    SourceKind, StreamLayout,
};
use mineral_persist::{ServerStore, ShelfFileRow};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::index::{playlist_detail_from_rows, playlists_from_rows, row_to_song};

/// shelf channel:本地音频索引的只读投影 + 播放解析。
pub struct ShelfChannel {
    /// persist 句柄(经 `.shelf()` 取索引视图)。
    store: ServerStore,
}

impl ShelfChannel {
    /// 新建 shelf channel。
    ///
    /// # Params:
    ///   - `store`: server 拥有的 persist 句柄
    ///
    /// # Return:
    ///   shelf channel 实例。
    pub fn new(store: ServerStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl MusicChannel for ShelfChannel {
    fn source(&self) -> SourceKind {
        SourceKind::SHELF
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            // 全局搜索里露出 shelf(内存 nucleo);browse `/` 另有更强的带拼音过滤。
            .searchable(vec![SearchKind::Song])
            // 写操作(建/删歌单等)shelf 不支持——它是「我声明怎么组织」,不是远端可写库。
            .playlist_edit(false)
            .artist_sections(ArtistSections::new(vec![ArtistSectionKind::Albums]))
            // 本地文件无网页形态。
            .build()
    }

    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>> {
        let shelf = self.store.shelf();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(row) = shelf.get(id.value()).await.map_err(Error::Other)? {
                out.push(row_to_song(&row));
            }
        }
        Ok(out)
    }

    async fn song_urls(&self, ids: &[SongId], _quality: BitRate) -> Result<Vec<PlayUrl>> {
        // quality 语义反转:不按档取流(本地文件就一份),而是报告文件实际是哪档。入参忽略。
        let shelf = self.store.shelf();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(row) = shelf.get(id.value()).await.map_err(Error::Other)? {
                out.push(build_play_url(&row));
            }
        }
        Ok(out)
    }

    async fn search_songs(&self, query: &str, page: Page) -> Result<SearchHits<Song>> {
        // 本地量小:全量加载进内存做 fuzzy(裸 nucleo,无拼音;拼音在 browse `/`)。
        let rows = self.store.shelf().list_all().await.map_err(Error::Other)?;
        let songs = rows.iter().map(row_to_song).collect::<Vec<Song>>();
        Ok(fuzzy_search(query, songs, page))
    }

    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        // 默认约定:每个含音频的目录一张歌单(spec §4)。organize 覆盖尚未接入。
        let rows = self.store.shelf().list_all().await.map_err(Error::Other)?;
        Ok(playlists_from_rows(&rows))
    }

    async fn playlist_detail(&self, id: &PlaylistId) -> Result<Playlist> {
        // 歌单 = 目录:裸值即目录路径,筛出其直接含的曲目。
        let rows = self.store.shelf().list_all().await.map_err(Error::Other)?;
        Ok(playlist_detail_from_rows(&rows, id.value()))
    }
}

/// 从索引行直构播放 URL(本地文件,`MediaUrl::Local`)。
///
/// # Params:
///   - `row`: 索引行
///
/// # Return:
///   填好 format / bitrate / 位深 / size / quality 的 [`PlayUrl`]。
fn build_play_url(row: &ShelfFileRow) -> PlayUrl {
    let format = row.format.clone().map(AudioFormat::from);
    let bit_depth = row.bit_depth.and_then(|b| u8::try_from(b).ok());
    let bitrate_bps = row
        .bitrate_kbps
        .and_then(|k| u32::try_from(k).ok())
        .and_then(|k| k.checked_mul(1000));
    PlayUrl {
        song_id: SongId::new(SourceKind::SHELF, row.uuid.as_str()),
        url: MediaUrl::Local(PathBuf::from(&row.path)),
        bitrate_bps,
        quality: derive_quality(format.as_ref(), bit_depth),
        size: row.size.and_then(|s| u64::try_from(s).ok()),
        format,
        bit_depth,
        stream_headers: Vec::new(),
        // 本地文件恒可 seek。
        layout: StreamLayout::Contiguous,
        substituted: false,
    }
}

/// 由文件实际格式 / 位深归档一个 [`BitRate`](报告用,非请求档)。
///
/// 无损容器:24bit+ 视作 Hires,否则 Lossless;有损 MVP 就近归 Exhigh(暂不按码率细分)。
///
/// # Params:
///   - `format`: 文件格式
///   - `bit_depth`: 位深
///
/// # Return:
///   归档的音质档。
fn derive_quality(format: Option<&AudioFormat>, bit_depth: Option<u8>) -> BitRate {
    match format {
        Some(fmt) if fmt.is_lossless() => {
            if bit_depth.is_some_and(|b| b >= 24) {
                BitRate::Hires
            } else {
                BitRate::Lossless
            }
        }
        _ => BitRate::Exhigh,
    }
}

/// 一首歌的匹配 haystack:曲名 + 艺人 + 专辑拼一起。
///
/// # Params:
///   - `song`: 歌曲
///
/// # Return:
///   拼接文本。
fn song_haystack(song: &Song) -> String {
    let mut haystack = song.name.clone();
    for artist in &song.artists {
        haystack.push(' ');
        haystack.push_str(&artist.name);
    }
    if let Some(album) = &song.album {
        haystack.push(' ');
        haystack.push_str(&album.name);
    }
    haystack
}

/// 内存 fuzzy 搜索:空 query 返回全部,否则按 nucleo 分数降序;再分页。
///
/// # Params:
///   - `query`: 查询串
///   - `songs`: 候选(全库)
///   - `page`: 分页
///
/// # Return:
///   命中页(`has_more` 精确来自剩余量)。
fn fuzzy_search(query: &str, songs: Vec<Song>, page: Page) -> SearchHits<Song> {
    let trimmed = query.trim();
    let ranked: Vec<Song> = if trimmed.is_empty() {
        songs
    } else {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(trimmed, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(u32, Song)> = songs
            .into_iter()
            .filter_map(|song| {
                let haystack = song_haystack(&song);
                let mut buf = Vec::new();
                let view = Utf32Str::new(&haystack, &mut buf);
                pattern.score(view, &mut matcher).map(|score| (score, song))
            })
            .collect();
        // 分数降序;同分按曲名稳定,避免库文件顺序抖动。
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        scored.into_iter().map(|(_, song)| song).collect()
    };
    paginate(ranked, page)
}

/// 按 [`Page`] 切片,`has_more` 由剩余量精确给出。
///
/// # Params:
///   - `items`: 已排序候选
///   - `page`: 分页
///
/// # Return:
///   该页命中。
fn paginate(items: Vec<Song>, page: Page) -> SearchHits<Song> {
    let offset = usize::try_from(page.offset).unwrap_or(0);
    let limit = usize::try_from(page.limit).unwrap_or(usize::MAX);
    let total = items.len();
    let paged: Vec<Song> = items.into_iter().skip(offset).take(limit).collect();
    let has_more = offset.saturating_add(paged.len()) < total;
    SearchHits::new(paged, has_more)
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::{MusicChannel, Page};
    use mineral_model::{BitRate, MediaUrl, PlaylistId, SongId, SourceKind};
    use mineral_persist::{ServerStore, ShelfFileRow};

    use super::ShelfChannel;

    /// upsert 一条指定路径的行(分组测试用)。
    async fn seed_at(store: &ServerStore, uuid: &str, path: &str) -> color_eyre::Result<()> {
        store
            .shelf()
            .upsert(&ShelfFileRow {
                uuid: uuid.to_owned(),
                mount: "/music".to_owned(),
                path: path.to_owned(),
                size: Some(1),
                mtime_ms: Some(1),
                format: Some("flac".to_owned()),
                bitrate_kbps: None,
                bit_depth: None,
                duration_ms: None,
                title: Some(uuid.to_owned()),
                artist: None,
                album: None,
                album_artist: None,
                track_no: None,
                genre: None,
            })
            .await
    }

    /// 造并 upsert 一条索引行,返回它的 SongId。
    async fn seed(
        store: &ServerStore,
        uuid: &str,
        title: &str,
        artist: &str,
    ) -> color_eyre::Result<SongId> {
        store
            .shelf()
            .upsert(&ShelfFileRow {
                uuid: uuid.to_owned(),
                mount: "/music".to_owned(),
                path: format!("/music/{uuid}.flac"),
                size: Some(2048),
                mtime_ms: Some(1_700_000_000_000),
                format: Some("flac".to_owned()),
                bitrate_kbps: Some(900),
                bit_depth: Some(24),
                duration_ms: Some(240_000),
                title: Some(title.to_owned()),
                artist: Some(artist.to_owned()),
                album: Some("专辑".to_owned()),
                album_artist: None,
                track_no: Some(1),
                genre: None,
            })
            .await?;
        Ok(SongId::new(SourceKind::SHELF, uuid))
    }

    /// source / caps:SHELF、可搜歌、不可写歌单。
    #[test]
    fn source_and_caps() {
        let ch = ShelfChannel::new(ServerStore::disabled());
        assert_eq!(ch.source(), SourceKind::SHELF);
        let caps = ch.caps();
        assert!(!caps.playlist_edit());
        assert!(!caps.searchable().is_empty(), "shelf 露在全局搜索");
    }

    /// songs_detail 点查:命中的还原成 Song,漏查的跳过。
    #[tokio::test]
    async fn songs_detail_points() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let id = seed(&store, "u1", "八匹马", "惘闻").await?;
        let ch = ShelfChannel::new(store);

        let missing = SongId::new(SourceKind::SHELF, "nope");
        let songs = ch.songs_detail(&[id.clone(), missing]).await?;
        assert_eq!(songs.len(), 1, "漏查的跳过");
        assert_eq!(songs.first().map(|s| s.name.as_str()), Some("八匹马"));
        Ok(())
    }

    /// song_urls:本地文件直构 PlayUrl(Local URL、24bit 无损归 Hires)。
    #[tokio::test]
    async fn song_urls_builds_local_playurl() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let id = seed(&store, "u1", "八匹马", "惘闻").await?;
        let ch = ShelfChannel::new(store);

        let urls = ch.song_urls(&[id], BitRate::Standard).await?;
        let pu = urls.first().ok_or_else(|| color_eyre::eyre::eyre!("应有一条"))?;
        assert!(matches!(pu.url, MediaUrl::Local(_)));
        assert_eq!(pu.quality, BitRate::Hires, "24bit 无损归 Hires");
        assert_eq!(pu.bitrate_bps, Some(900_000), "kbps→bps");
        Ok(())
    }

    /// search_songs:fuzzy 命中 + 分页;空 query 出全部。
    #[tokio::test]
    async fn search_fuzzy_and_paginate() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        seed(&store, "u1", "Palisade", "Mineral").await?;
        seed(&store, "u2", "夜間飛行", "惘闻").await?;
        seed(&store, "u3", "Parade", "Mineral").await?;
        let ch = ShelfChannel::new(store);

        // 空 query:全部三首。
        let all = ch.search_songs("", Page::new(0, 30)).await?;
        assert_eq!(all.items.len(), 3);

        // fuzzy "pale":命中 Palisade / Parade(子序列),不命中中文那首。
        let hits = ch.search_songs("pale", Page::new(0, 30)).await?;
        assert!(!hits.items.is_empty(), "至少命中 Palisade");
        assert!(
            hits.items.iter().all(|s| s.name != "夜間飛行"),
            "不相关的不命中"
        );

        // 分页:limit 1 → 第一页 1 条 + has_more。
        let first = ch.search_songs("", Page::new(0, 1)).await?;
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.has_more, Some(true));
        Ok(())
    }

    /// my_playlists 按目录分组(spec §4 默认约定);playlist_detail 出该目录曲目。
    #[tokio::test]
    async fn playlists_group_by_directory() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        seed_at(&store, "u1", "/music/albumA/1.flac").await?;
        seed_at(&store, "u2", "/music/albumA/2.flac").await?;
        seed_at(&store, "u3", "/music/albumB/1.flac").await?;
        let ch = ShelfChannel::new(store);

        let lists = ch.my_playlists().await?;
        assert_eq!(lists.len(), 2, "两个目录 → 两张歌单");
        assert!(lists.iter().all(|p| p.songs.is_empty()), "列表版不带曲目");

        let detail = ch
            .playlist_detail(&PlaylistId::new(SourceKind::SHELF, "/music/albumA"))
            .await?;
        assert_eq!(detail.songs.len(), 2, "albumA 两首");
        assert_eq!(detail.name, "albumA");
        Ok(())
    }

    /// 降级 store:search 空、detail 空,不报错。
    #[tokio::test]
    async fn disabled_store_is_empty() -> color_eyre::Result<()> {
        let ch = ShelfChannel::new(ServerStore::disabled());
        assert!(ch.search_songs("x", Page::new(0, 30)).await?.items.is_empty());
        assert!(
            ch.songs_detail(&[SongId::new(SourceKind::SHELF, "u1")])
                .await?
                .is_empty()
        );
        Ok(())
    }
}

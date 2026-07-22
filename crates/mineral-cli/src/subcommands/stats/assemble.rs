//! `stats report` / `top` 的装配:stats.db 一库直出(数值聚合 + 展示名)。
//!
//! 展示名不跨库:歌名 JOIN 库内 songs 维表,专辑 / 艺人 / 歌单名取 plays 行的
//! `context_name` 快照聚合。装配好的 [`RawReport`] 交 `mineral_stats::combine` 纯函数
//! 成形——与将来 TUI 盘点页经 daemon 出报告复用同一装配,不重复口径。

use std::ops::Range;

use mineral_stats::{
    ContextSlice, NamedEntry, RawReport, ReportOptions, StatsReport, StatsStore, TopBy, combine,
};

/// 装配一份完整盘点报告(§8.1 全套,名字随查询直出)。
///
/// # Params:
///   - `store`: stats.db 查询句柄
///   - `range`: 时间窗口 `[start_ms, end_ms)`
///   - `opts`: 有效播放阈值 + 榜长
///
/// # Return:
///   装配好的报告
pub async fn stats_report(
    store: &StatsStore,
    range: Range<i64>,
    opts: &ReportOptions,
) -> color_eyre::Result<StatsReport> {
    Ok(combine(raw_report(store, range, opts).await?))
}

/// 跑 §8.1 九项查询,拼成 [`RawReport`]。
async fn raw_report(
    store: &StatsStore,
    range: Range<i64>,
    opts: &ReportOptions,
) -> color_eyre::Result<RawReport> {
    Ok(RawReport {
        totals: store.totals(range.clone()).await?,
        top_songs: store.top_songs(range.clone(), TopBy::Plays, opts).await?,
        top_albums: store.top_albums(range.clone(), TopBy::Plays, opts).await?,
        top_artists: store.top_artists(range.clone(), TopBy::Plays, opts).await?,
        distributions: store.distributions(range.clone()).await?,
        hourly: store
            .listen_buckets(range.clone(), mineral_stats::BucketBy::Hour)
            .await?,
        discoveries: store.discoveries(range.clone(), opts.top_limit()).await?,
        endurance: store.endurance(range.clone()).await?,
        events: store.event_summary(range, opts.top_limit()).await?,
    })
}

/// `top` 的单榜类别
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum TopCategory {
    /// top 歌曲
    Songs,

    /// top 专辑(口味口径:按歌曲的 album_id 归属聚合——「听了哪张专辑的歌」)
    Albums,

    /// top 专辑页(context 口径:按起播语境聚合——「从谁的详情页起播」,与 `Albums`
    /// 是两条不同的统计路径,不要混为一谈)
    AlbumPages,

    /// top 艺人(口味口径:按歌曲的 artists 归属聚合——「听了谁的歌」)
    Artists,

    /// top 艺人页(context 口径:按起播语境聚合——「从谁的详情页起播」,与 `Artists`
    /// 是两条不同的统计路径,不要混为一谈)
    ArtistPages,

    /// top 歌单
    Playlists,
}

impl TopCategory {
    /// 文本渲染的榜标题(`"top songs"` 等;`render::render_top` 拼 `▸` 前缀,不带尾冒号)。
    pub fn text_title(self) -> &'static str {
        match self {
            Self::Songs => "top songs",
            Self::Albums => "top albums",
            Self::AlbumPages => "top album pages (by play-from-page count)",
            Self::Artists => "top artists",
            Self::ArtistPages => "top artist pages (by play-from-page count)",
            Self::Playlists => "top playlists",
        }
    }

    /// markdown 渲染的榜标题(`"Top 歌曲"` 等)。
    pub fn md_title(self) -> &'static str {
        match self {
            Self::Songs => "Top 歌曲",
            Self::Albums => "Top 专辑",
            Self::AlbumPages => "Top 专辑页(从详情页起播次数)",
            Self::Artists => "Top 艺人",
            Self::ArtistPages => "Top 艺人页(从详情页起播次数)",
            Self::Playlists => "Top 歌单",
        }
    }
}

/// 查一张 top 榜,统一成 `Vec<NamedEntry>`(各类别同一渲染形状,名字随查询直出)。
///
/// # Params:
///   - `store`: stats.db 查询句柄
///   - `category`: 榜类别
///   - `range`: 时间窗口
///   - `by`: 次数 / 时长口径(playlists 恒按次数)
///   - `opts`: 有效播放阈值 + 榜长
///
/// # Return:
///   带名榜项列表
pub async fn top_entries(
    store: &StatsStore,
    category: TopCategory,
    range: Range<i64>,
    by: TopBy,
    opts: &ReportOptions,
) -> color_eyre::Result<Vec<NamedEntry>> {
    let out = match category {
        TopCategory::Songs => store
            .top_songs(range, by, opts)
            .await?
            .into_iter()
            .map(|t| NamedEntry {
                id: t.song.qualified(),
                name: t.name,
                plays: t.plays,
                listen_ms: t.listen_ms,
            })
            .collect(),
        TopCategory::Albums => store
            .top_albums(range, by, opts)
            .await?
            .into_iter()
            .map(|t| NamedEntry {
                id: t.album.qualified(),
                name: t.name,
                plays: t.plays,
                listen_ms: t.listen_ms,
            })
            .collect(),
        TopCategory::Artists => store
            .top_artists(range, by, opts)
            .await?
            .into_iter()
            .map(|t| NamedEntry {
                id: t.artist.qualified(),
                name: t.name,
                plays: t.plays,
                listen_ms: t.listen_ms,
            })
            .collect(),
        TopCategory::AlbumPages => store
            .top_contexts(range, Some("album"), opts.min_listen_ms(), opts.top_limit())
            .await?
            .into_iter()
            .map(context_entry)
            .collect(),
        TopCategory::ArtistPages => store
            .top_contexts(
                range,
                Some("artist"),
                opts.min_listen_ms(),
                opts.top_limit(),
            )
            .await?
            .into_iter()
            .map(context_entry)
            .collect(),
        TopCategory::Playlists => store
            .top_contexts(
                range,
                Some("playlist"),
                opts.min_listen_ms(),
                opts.top_limit(),
            )
            .await?
            .into_iter()
            .map(context_entry)
            .collect(),
    };
    Ok(out)
}

/// 一条队列语境 → 榜项(`Playlists` / `AlbumPages` / `ArtistPages` 共用):id 用
/// `context_ref`(qualified 串;无引用回落 `manual`),名取组内 `context_name` 快照。
fn context_entry(slice: ContextSlice) -> NamedEntry {
    NamedEntry {
        id: slice.reference.unwrap_or_else(|| "manual".to_owned()),
        name: slice.name,
        plays: slice.plays,
        listen_ms: slice.listen_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::{TopCategory, stats_report, top_entries};
    use mineral_model::{PlaylistId, SourceKind};
    use mineral_stats::{
        Actor, FinishReason, PlayAudioSnapshot, PlayMode, PlayOrigin, PlayRecord, PlaybackOrigin,
        QueueContext, ReportOptions, StatsStore, TopBy,
    };

    /// 造一行播放事实(歌单语境带名快照)。
    fn play_record(session_id: i64) -> PlayRecord {
        PlayRecord {
            song_id: mineral_test::song("42").id,
            started_at: 1000,
            ended_at: 61_000,
            listen_ms: 60_000,
            duration_ms_snapshot: None,
            finish_reason: FinishReason::Eof,
            skip_at_ms: None,
            play_mode: PlayMode::Sequential,
            session_id,
            origin: PlayOrigin::Explicit,
            actor: Actor::User,
            context: QueueContext::Playlist {
                id: PlaylistId::new(SourceKind::NETEASE, "7"),
                name: Some("收藏夹".to_owned()),
            },
            audio: PlayAudioSnapshot::default(),
            playback_origin: PlaybackOrigin::Remote,
        }
    }

    /// 自足性:目录里只有 stats.db(无任何其他数据库),报告与单榜均带名——歌名出自
    /// 库内维表,歌单名出自 `context_name` 快照。
    #[tokio::test]
    async fn report_names_come_from_stats_db_alone() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        let sid = store
            .open_session(1000)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected session id"))?;
        store
            .upsert_song(&mineral_test::with_name(mineral_test::song("42"), "栞"))
            .await?;
        store.record_play(&play_record(sid)).await?;
        let opts = ReportOptions::builder()
            .min_listen_ms(0)
            .top_limit(10)
            .build();

        let report = stats_report(&store, 0..i64::MAX, &opts).await?;
        let first = report
            .top_songs
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 top 歌"))?;
        assert_eq!(first.name.as_deref(), Some("栞"), "歌名出自库内维表");

        let playlists = top_entries(
            &store,
            TopCategory::Playlists,
            0..i64::MAX,
            TopBy::Plays,
            &opts,
        )
        .await?;
        let pl = playlists
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 top 歌单"))?;
        assert_eq!(pl.id, "netease:7");
        assert_eq!(
            pl.name.as_deref(),
            Some("收藏夹"),
            "歌单名出自 context_name 快照"
        );
        Ok(())
    }

    /// ArtistPages(context 口径「从谁的详情页起播」)与 Artists(口味口径「听了谁的歌」)
    /// 是两条不同的查询路径:前者走 `top_contexts(Some("artist"))`,只认起播语境,
    /// 不依赖 song_artists 维表,即便歌从未 upsert_song 富化过也照样计入。
    #[tokio::test]
    async fn artist_pages_category_uses_context_ref() -> color_eyre::Result<()> {
        use mineral_model::{ArtistId, SourceKind};

        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        let sid = store
            .open_session(1000)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected session id"))?;
        let mut record = play_record(sid);
        record.context = QueueContext::Artist {
            id: ArtistId::new(SourceKind::NETEASE, "art"),
            name: Some("从详情页起播的艺人".to_owned()),
        };
        store.record_play(&record).await?;
        let opts = ReportOptions::builder()
            .min_listen_ms(0)
            .top_limit(10)
            .build();

        let pages = top_entries(
            &store,
            TopCategory::ArtistPages,
            0..i64::MAX,
            TopBy::Plays,
            &opts,
        )
        .await?;
        let only = pages
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 top 艺人页"))?;
        assert_eq!(only.id, "netease:art");
        assert_eq!(only.name.as_deref(), Some("从详情页起播的艺人"));
        Ok(())
    }

    /// AlbumPages(context 口径「从谁的详情页起播」)与 Albums(口味口径「听了哪张专辑的
    /// 歌」)是两条不同的查询路径:前者走 `top_contexts(Some("album"))`,只认起播语境,
    /// 不依赖 songs 维表的 album_id,即便歌从未 upsert_song 富化过也照样计入。
    #[tokio::test]
    async fn album_pages_category_uses_context_ref() -> color_eyre::Result<()> {
        use mineral_model::{AlbumId, SourceKind};

        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        let sid = store
            .open_session(1000)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected session id"))?;
        let mut record = play_record(sid);
        record.context = QueueContext::Album {
            id: AlbumId::new(SourceKind::NETEASE, "al"),
            name: Some("从详情页起播的专辑".to_owned()),
        };
        store.record_play(&record).await?;
        let opts = ReportOptions::builder()
            .min_listen_ms(0)
            .top_limit(10)
            .build();

        let pages = top_entries(
            &store,
            TopCategory::AlbumPages,
            0..i64::MAX,
            TopBy::Plays,
            &opts,
        )
        .await?;
        let only = pages
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 top 专辑页"))?;
        assert_eq!(only.id, "netease:al");
        assert_eq!(only.name.as_deref(), Some("从详情页起播的专辑"));
        Ok(())
    }
}

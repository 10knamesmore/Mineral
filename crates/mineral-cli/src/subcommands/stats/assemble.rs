//! `stats report` / `top` 的装配:stats.db 数值聚合 + mineral.db 名字回查。
//!
//! CLI 离线直读两库(不经 daemon):数值全在 stats.db,展示名由 mineral.db 的 `song_meta` /
//! `song_artists` / `playlist_cache` 回查补齐。装配好的 [`RawReport`] 交 `mineral_stats::combine`
//! 纯函数落名——与将来 TUI 盘点页经 daemon 出报告复用同一装配,不重复口径。

use std::ops::Range;

use mineral_model::{AlbumId, ArtistId, PlaylistId, SongId, SourceKind};
use mineral_persist::ServerStore;
use mineral_stats::{
    ContextSlice, NamedEntry, RawReport, ReportOptions, StatsReport, StatsStore, TopBy, combine,
};
use rustc_hash::FxHashMap;

/// mineral.db 名字回查器(按 id 内在的 namespace 选 scope,离线只读)。
pub struct NameResolver {
    /// mineral.db 句柄(降级句柄下各查询恒 `None`)。
    persist: ServerStore,
}

impl NameResolver {
    /// 绑定一个已打开的 mineral.db 句柄。
    pub fn new(persist: ServerStore) -> Self {
        Self { persist }
    }

    /// 歌名(`song_meta.name`);未命中 `None`。
    pub async fn song(&self, id: &SongId) -> color_eyre::Result<Option<String>> {
        Ok(self
            .persist
            .scope(id.namespace())
            .get_meta(id)
            .await?
            .map(|s| s.name))
    }

    /// 专辑名(任一成员歌的 `album_name`);未命中 `None`。
    pub async fn album(&self, id: &AlbumId) -> color_eyre::Result<Option<String>> {
        self.persist.scope(id.namespace()).album_name(id).await
    }

    /// 艺名(任一署名行);未命中 `None`。
    pub async fn artist(&self, id: &ArtistId) -> color_eyre::Result<Option<String>> {
        self.persist.scope(id.namespace()).artist_name(id).await
    }

    /// 歌单名(`playlist_cache.name`);未命中 `None`。
    pub async fn playlist(&self, id: &PlaylistId) -> color_eyre::Result<Option<String>> {
        Ok(self
            .persist
            .scope(id.namespace())
            .get_playlist_cache(id)
            .await?
            .and_then(|p| p.name))
    }
}

/// 装配一份带展示名的完整盘点报告(§8.1 全套 + mineral.db 回查名)。
///
/// # Params:
///   - `store`: stats.db 查询句柄
///   - `resolver`: mineral.db 名字回查器
///   - `range`: 时间窗口 `[start_ms, end_ms)`
///   - `opts`: 有效播放阈值 + 榜长
///
/// # Return:
///   落名后的报告
pub async fn stats_report(
    store: &StatsStore,
    resolver: &NameResolver,
    range: Range<i64>,
    opts: &ReportOptions,
) -> color_eyre::Result<StatsReport> {
    let raw = raw_report(store, range, opts).await?;
    let names = report_names(resolver, &raw).await?;
    Ok(combine(raw, &names))
}

/// 跑 §8.1 九项查询,拼成未落名的 [`RawReport`]。
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

/// 收集 top 歌 / 专辑 / 艺人的 qualified id,逐一回查名,建 `qualified id → 名` 映射
/// (缺名的不进映射,`combine` 自动回落 id)。
async fn report_names(
    resolver: &NameResolver,
    raw: &RawReport,
) -> color_eyre::Result<FxHashMap<String, String>> {
    let mut names = FxHashMap::<String, String>::default();
    for t in &raw.top_songs {
        if let Some(name) = resolver.song(&t.song).await? {
            names.insert(t.song.qualified(), name);
        }
    }
    for t in &raw.top_albums {
        if let Some(name) = resolver.album(&t.album).await? {
            names.insert(t.album.qualified(), name);
        }
    }
    for t in &raw.top_artists {
        if let Some(name) = resolver.artist(&t.artist).await? {
            names.insert(t.artist.qualified(), name);
        }
    }
    Ok(names)
}

/// `top` 的单榜类别(§8.2:`playlists` = 队列上下文口径)。
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum TopCategory {
    /// top 歌曲。
    Songs,

    /// top 专辑(专辑语境聚合)。
    Albums,

    /// top 艺人(艺人语境聚合)。
    Artists,

    /// top 歌单(队列上下文口径:最常从哪个歌单起播)。
    Playlists,
}

impl TopCategory {
    /// 文本渲染的榜标题行(`"top songs:"` 等)。
    pub fn text_title(self) -> &'static str {
        match self {
            Self::Songs => "top songs:",
            Self::Albums => "top albums:",
            Self::Artists => "top artists:",
            Self::Playlists => "top playlists:",
        }
    }

    /// markdown 渲染的榜标题(`"Top 歌曲"` 等)。
    pub fn md_title(self) -> &'static str {
        match self {
            Self::Songs => "Top 歌曲",
            Self::Albums => "Top 专辑",
            Self::Artists => "Top 艺人",
            Self::Playlists => "Top 歌单",
        }
    }
}

/// 查一张 top 榜并落展示名,统一成 `Vec<NamedEntry>`(四类别同一渲染形状)。
///
/// # Params:
///   - `store`: stats.db 查询句柄
///   - `resolver`: mineral.db 名字回查器
///   - `category`: 榜类别
///   - `range`: 时间窗口
///   - `by`: 次数 / 时长口径(playlists 恒按次数)
///   - `opts`: 有效播放阈值 + 榜长
///
/// # Return:
///   带名榜项列表
pub async fn top_entries(
    store: &StatsStore,
    resolver: &NameResolver,
    category: TopCategory,
    range: Range<i64>,
    by: TopBy,
    opts: &ReportOptions,
) -> color_eyre::Result<Vec<NamedEntry>> {
    let mut out = Vec::<NamedEntry>::new();
    match category {
        TopCategory::Songs => {
            for t in store.top_songs(range, by, opts).await? {
                let name = resolver.song(&t.song).await?;
                out.push(NamedEntry {
                    id: t.song.qualified(),
                    name,
                    plays: t.plays,
                    listen_ms: t.listen_ms,
                });
            }
        }
        TopCategory::Albums => {
            for t in store.top_albums(range, by, opts).await? {
                let name = resolver.album(&t.album).await?;
                out.push(NamedEntry {
                    id: t.album.qualified(),
                    name,
                    plays: t.plays,
                    listen_ms: t.listen_ms,
                });
            }
        }
        TopCategory::Artists => {
            for t in store.top_artists(range, by, opts).await? {
                let name = resolver.artist(&t.artist).await?;
                out.push(NamedEntry {
                    id: t.artist.qualified(),
                    name,
                    plays: t.plays,
                    listen_ms: t.listen_ms,
                });
            }
        }
        TopCategory::Playlists => {
            let list = store
                .top_contexts(range, Some("playlist"), opts.top_limit())
                .await?;
            for c in list {
                out.push(playlist_entry(resolver, c).await?);
            }
        }
    }
    Ok(out)
}

/// 一条 playlist 语境 → 带名榜项:`context_ref` 是 qualified `PlaylistId`,回查歌单名;
/// 坏格式 / 无引用回落 id 串(或 `manual`)。
async fn playlist_entry(
    resolver: &NameResolver,
    slice: ContextSlice,
) -> color_eyre::Result<NamedEntry> {
    let (id, name) = match slice.reference.as_deref().and_then(split_playlist_id) {
        Some(pid) => {
            let name = resolver.playlist(&pid).await?;
            (pid.qualified(), name)
        }
        None => (slice.reference.unwrap_or_else(|| "manual".to_owned()), None),
    };
    Ok(NamedEntry {
        id,
        name,
        plays: slice.plays,
        listen_ms: slice.listen_ms,
    })
}

/// 从 `context_ref` 的 qualified 串(`name:value`)重建 [`PlaylistId`];无 `:` 则 `None`。
fn split_playlist_id(reference: &str) -> Option<PlaylistId> {
    let (ns, value) = reference.split_once(':')?;
    Some(PlaylistId::new(SourceKind::from_name(ns), value))
}

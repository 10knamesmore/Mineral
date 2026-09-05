//! 系统域事件的落库:每张表一条运行期 `query`(无 actor 列)。

use color_eyre::eyre::WrapErr as _;
use mineral_model::SongId;
use sea_orm::{DatabaseConnection, EntityTrait, Set};

use crate::event::SystemEvent;
use crate::{
    CacheHarvestOutcome, FailOpen, GaplessResult, HookDecision, HookKind, HookStage,
    PrefetchResolution, PrefetchSource, ScriptEvent, StreamOutcome,
};

/// 系统域每张表都有的公共写入列 + live 连接句柄(无 actor)。
struct SystemWrite<'a> {
    /// live 连接池。
    pool: &'a DatabaseConnection,

    /// 事件时刻 epoch ms。
    ts: i64,

    /// 归属会话 id;无会话上下文落 NULL。
    session_id: Option<i64>,
}

/// 拆 `Option<SongId>` 成 `(ns, song_value)` 两列;`None` 时两列皆 NULL。
fn split_song(song: Option<&SongId>) -> (Option<&str>, Option<&str>) {
    match song {
        Some(s) => (Some(s.namespace().name()), Some(s.value())),
        None => (None, None),
    }
}

/// 按变体把一条系统域事件落到对应表。公共列(ts / session_id)在此汇入。
pub(super) async fn write(
    pool: &DatabaseConnection,
    ts: i64,
    session_id: Option<i64>,
    event: &SystemEvent,
) -> color_eyre::Result<()> {
    let w = SystemWrite {
        pool,
        ts,
        session_id,
    };
    match event {
        SystemEvent::StreamResolution {
            song,
            quality_requested,
            outcome,
            for_prefetch,
        } => write_stream_resolution(&w, song, quality_requested, *outcome, *for_prefetch).await,
        SystemEvent::HookFire {
            song,
            hook,
            stage,
            decision,
            fail_open,
        } => write_hook_fire(&w, song.as_ref(), *hook, *stage, *decision, *fail_open).await,
        SystemEvent::GaplessBoundary { song, result } => {
            write_gapless_boundary(&w, song, *result).await
        }
        SystemEvent::Prefetch {
            song,
            source,
            resolution,
        } => write_prefetch(&w, song, *source, *resolution).await,
        SystemEvent::CacheHarvest {
            song,
            quality,
            format,
            outcome,
            bytes,
        } => write_cache_harvest(&w, song, quality, format, *outcome, *bytes).await,
        SystemEvent::CacheEviction { cache_key, bytes } => {
            write_cache_eviction(&w, cache_key, *bytes).await
        }
        SystemEvent::ScriptLifecycle { event, detail } => {
            write_script_lifecycle(&w, *event, detail.as_deref()).await
        }
        SystemEvent::ConfigReload => write_config_reload(&w).await,
    }
}

/// 落 stream_resolutions 一行。
async fn write_stream_resolution(
    w: &SystemWrite<'_>,
    song: &SongId,
    quality_requested: &str,
    outcome: StreamOutcome,
    for_prefetch: bool,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    let for_prefetch = i64::from(for_prefetch);
    crate::entity::stream_resolutions::Entity::insert(
        crate::entity::stream_resolutions::ActiveModel {
            ts: Set(w.ts),
            session_id: Set(w.session_id),
            ns: Set(ns.to_owned()),
            song_value: Set(song_value.to_owned()),
            quality_requested: Set(quality_requested.to_owned()),
            outcome: Set(outcome),
            for_prefetch: Set(for_prefetch),
            ..Default::default()
        },
    )
    .exec(w.pool)
    .await
    .wrap_err("record_event stream_resolutions 落库失败")?;
    Ok(())
}

/// 落 hook_fires 一行。
async fn write_hook_fire(
    w: &SystemWrite<'_>,
    song: Option<&SongId>,
    hook: HookKind,
    stage: HookStage,
    decision: HookDecision,
    fail_open: Option<FailOpen>,
) -> color_eyre::Result<()> {
    let (ns, song_value) = split_song(song);
    crate::entity::hook_fires::Entity::insert(crate::entity::hook_fires::ActiveModel {
        ts: Set(w.ts),
        session_id: Set(w.session_id),
        ns: Set(ns.map(str::to_owned)),
        song_value: Set(song_value.map(str::to_owned)),
        hook: Set(hook),
        stage: Set(stage),
        decision: Set(decision),
        fail_open: Set(fail_open),
        ..Default::default()
    })
    .exec(w.pool)
    .await
    .wrap_err("record_event hook_fires 落库失败")?;
    Ok(())
}

/// 落 gapless_boundaries 一行。
async fn write_gapless_boundary(
    w: &SystemWrite<'_>,
    song: &SongId,
    result: GaplessResult,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    crate::entity::gapless_boundaries::Entity::insert(
        crate::entity::gapless_boundaries::ActiveModel {
            ts: Set(w.ts),
            session_id: Set(w.session_id),
            ns: Set(ns.to_owned()),
            song_value: Set(song_value.to_owned()),
            result: Set(result),
            ..Default::default()
        },
    )
    .exec(w.pool)
    .await
    .wrap_err("record_event gapless_boundaries 落库失败")?;
    Ok(())
}

/// 落 prefetches 一行。
async fn write_prefetch(
    w: &SystemWrite<'_>,
    song: &SongId,
    source: PrefetchSource,
    resolution: PrefetchResolution,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    crate::entity::prefetches::Entity::insert(crate::entity::prefetches::ActiveModel {
        ts: Set(w.ts),
        session_id: Set(w.session_id),
        ns: Set(ns.to_owned()),
        song_value: Set(song_value.to_owned()),
        source: Set(source),
        resolution: Set(resolution),
        ..Default::default()
    })
    .exec(w.pool)
    .await
    .wrap_err("record_event prefetches 落库失败")?;
    Ok(())
}

/// 落 cache_harvests 一行。
async fn write_cache_harvest(
    w: &SystemWrite<'_>,
    song: &SongId,
    quality: &str,
    format: &str,
    outcome: CacheHarvestOutcome,
    bytes: Option<i64>,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    crate::entity::cache_harvests::Entity::insert(crate::entity::cache_harvests::ActiveModel {
        ts: Set(w.ts),
        session_id: Set(w.session_id),
        ns: Set(ns.to_owned()),
        song_value: Set(song_value.to_owned()),
        quality: Set(quality.to_owned()),
        format: Set(format.to_owned()),
        outcome: Set(outcome),
        bytes: Set(bytes),
        ..Default::default()
    })
    .exec(w.pool)
    .await
    .wrap_err("record_event cache_harvests 落库失败")?;
    Ok(())
}

/// 落 cache_evictions 一行。
async fn write_cache_eviction(
    w: &SystemWrite<'_>,
    cache_key: &str,
    bytes: i64,
) -> color_eyre::Result<()> {
    crate::entity::cache_evictions::Entity::insert(crate::entity::cache_evictions::ActiveModel {
        ts: Set(w.ts),
        session_id: Set(w.session_id),
        cache_key: Set(cache_key.to_owned()),
        bytes: Set(bytes),
        ..Default::default()
    })
    .exec(w.pool)
    .await
    .wrap_err("record_event cache_evictions 落库失败")?;
    Ok(())
}

/// 落 script_lifecycle 一行。
async fn write_script_lifecycle(
    w: &SystemWrite<'_>,
    event: ScriptEvent,
    detail: Option<&str>,
) -> color_eyre::Result<()> {
    crate::entity::script_lifecycle::Entity::insert(crate::entity::script_lifecycle::ActiveModel {
        ts: Set(w.ts),
        session_id: Set(w.session_id),
        event: Set(event),
        detail: Set(detail.map(str::to_owned)),
        ..Default::default()
    })
    .exec(w.pool)
    .await
    .wrap_err("record_event script_lifecycle 落库失败")?;
    Ok(())
}

/// 落 config_reloads 一行(无专有列)。
async fn write_config_reload(w: &SystemWrite<'_>) -> color_eyre::Result<()> {
    crate::entity::config_reloads::Entity::insert(crate::entity::config_reloads::ActiveModel {
        ts: Set(w.ts),
        session_id: Set(w.session_id),
        ..Default::default()
    })
    .exec(w.pool)
    .await
    .wrap_err("record_event config_reloads 落库失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::event::{
        CacheHarvestOutcome, FailOpen, GaplessResult, HookDecision, HookKind, HookStage,
        PrefetchResolution, PrefetchSource, StatsEvent, StreamOutcome, SystemEvent,
    };
    use crate::store::StatsStore;
    use mineral_model::{SongId, SourceKind};
    use sea_orm::{DatabaseConnection, EntityTrait, QueryOrder, QuerySelect};

    /// 建落盘临时库(系统域事件 session_id 直接落 NULL,免开会话)。
    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    /// 从句柄取 live pool。
    fn live(store: &StatsStore) -> color_eyre::Result<&DatabaseConnection> {
        store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected live pool"))
    }

    fn song() -> SongId {
        SongId::new(SourceKind::NETEASE, "42")
    }

    /// stream_resolutions:empty 与 error 两种 outcome 落库后可区分。
    #[tokio::test]
    async fn record_stream_resolution_empty_vs_error() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let empty = StatsEvent::System(SystemEvent::StreamResolution {
            song: song(),
            quality_requested: "lossless".to_owned(),
            outcome: StreamOutcome::Empty,
            for_prefetch: false,
        });
        let error = StatsEvent::System(SystemEvent::StreamResolution {
            song: song(),
            quality_requested: "lossless".to_owned(),
            outcome: StreamOutcome::Error,
            for_prefetch: true,
        });
        store.record_event(1000, None, &empty).await?;
        store.record_event(1001, None, &error).await?;

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            outcome: StreamOutcome,
            for_prefetch: i64,
        }
        let rows = crate::entity::stream_resolutions::Entity::find()
            .select_only()
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::stream_resolutions::Column::Outcome,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::stream_resolutions::Column::ForPrefetch,
            ))
            .order_by_asc(sea_orm::sea_query::Expr::col(
                crate::entity::stream_resolutions::Column::Id,
            ))
            .into_model::<Row>()
            .all(live(&store)?)
            .await?;
        let first = rows
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected first row"))?;
        let second = rows
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("expected second row"))?;
        assert_eq!(first.outcome, StreamOutcome::Empty);
        assert_eq!(first.for_prefetch, 0);
        assert_eq!(second.outcome, StreamOutcome::Error);
        assert_eq!(second.for_prefetch, 1);
        Ok(())
    }

    /// hook_fires:无关具体曲(ns / song_value NULL)、fail_open 落值、decision=continue。
    #[tokio::test]
    async fn record_hook_fire_no_song_fail_open() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let event = StatsEvent::System(SystemEvent::HookFire {
            song: None,
            hook: HookKind::BeforeStream,
            stage: HookStage::Immediate,
            decision: HookDecision::Continue,
            fail_open: Some(FailOpen::Timeout),
        });
        store.record_event(2000, None, &event).await?;

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            ns: Option<String>,
            song_value: Option<String>,
            hook: HookKind,
            decision: HookDecision,
            fail_open: Option<FailOpen>,
        }
        let row = crate::entity::hook_fires::Entity::find()
            .select_only()
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::hook_fires::Column::Ns,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::hook_fires::Column::SongValue,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::hook_fires::Column::Hook,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::hook_fires::Column::Decision,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::hook_fires::Column::FailOpen,
            ))
            .into_model::<Row>()
            .one(live(&store)?)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected database row"))?;
        assert_eq!(row.ns, None);
        assert_eq!(row.song_value, None);
        assert_eq!(row.hook, HookKind::BeforeStream);
        assert_eq!(row.decision, HookDecision::Continue);
        assert_eq!(row.fail_open, Some(FailOpen::Timeout));
        Ok(())
    }

    /// gapless_boundaries:adopt 与 fallback 两种 result 各落一行、可区分。
    #[tokio::test]
    async fn record_gapless_adopt_and_fallback() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        store
            .record_event(
                1000,
                None,
                &StatsEvent::System(SystemEvent::GaplessBoundary {
                    song: song(),
                    result: GaplessResult::Adopt,
                }),
            )
            .await?;
        store
            .record_event(
                1001,
                None,
                &StatsEvent::System(SystemEvent::GaplessBoundary {
                    song: song(),
                    result: GaplessResult::Fallback,
                }),
            )
            .await?;

        let results = crate::entity::gapless_boundaries::Entity::find()
            .select_only()
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::gapless_boundaries::Column::Result,
            ))
            .order_by_asc(sea_orm::sea_query::Expr::col(
                crate::entity::gapless_boundaries::Column::Id,
            ))
            .into_tuple::<(GaplessResult,)>()
            .all(live(&store)?)
            .await?;
        let first = results
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected first row"))?;
        let second = results
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("expected second row"))?;
        assert_eq!(first.0, GaplessResult::Adopt);
        assert_eq!(second.0, GaplessResult::Fallback);
        Ok(())
    }

    /// cache_evictions:cache_key + bytes 原样落。
    #[tokio::test]
    async fn record_cache_eviction() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let event = StatsEvent::System(SystemEvent::CacheEviction {
            cache_key: "netease:42:lossless".to_owned(),
            bytes: 8_388_608,
        });
        store.record_event(3000, None, &event).await?;

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            cache_key: String,
            bytes: i64,
        }
        let row = crate::entity::cache_evictions::Entity::find()
            .select_only()
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::cache_evictions::Column::CacheKey,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::cache_evictions::Column::Bytes,
            ))
            .into_model::<Row>()
            .one(live(&store)?)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected database row"))?;
        assert_eq!(row.cache_key, "netease:42:lossless");
        assert_eq!(row.bytes, 8_388_608);
        Ok(())
    }

    /// prefetches:song 拆两列、source / resolution 各落值(armed / vetoed / rewritten / failed 四态)。
    #[tokio::test]
    async fn record_prefetch_resolutions() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let cases = [
            (PrefetchSource::Remote, PrefetchResolution::Armed),
            (PrefetchSource::Local, PrefetchResolution::Vetoed),
            (PrefetchSource::Remote, PrefetchResolution::Rewritten),
            (PrefetchSource::Local, PrefetchResolution::Failed),
        ];
        for (i, (source, resolution)) in cases.iter().enumerate() {
            store
                .record_event(
                    4000 + i64::try_from(i)?,
                    None,
                    &StatsEvent::System(SystemEvent::Prefetch {
                        song: song(),
                        source: *source,
                        resolution: *resolution,
                    }),
                )
                .await?;
        }

        let rows = crate::entity::prefetches::Entity::find()
            .select_only()
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::prefetches::Column::Source,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::prefetches::Column::Resolution,
            ))
            .order_by_asc(sea_orm::sea_query::Expr::col(
                crate::entity::prefetches::Column::Id,
            ))
            .into_tuple::<(PrefetchSource, PrefetchResolution)>()
            .all(live(&store)?)
            .await?;
        assert_eq!(rows, cases);
        Ok(())
    }

    /// cache_harvests:quality / format / outcome / bytes 各列;format=unknown(源未声明)
    /// 与 bytes=None(丢弃)也能落。
    #[tokio::test]
    async fn record_cache_harvest_cached_and_discarded() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        store
            .record_event(
                5000,
                None,
                &StatsEvent::System(SystemEvent::CacheHarvest {
                    song: song(),
                    quality: "lossless".to_owned(),
                    format: "flac".to_owned(),
                    outcome: CacheHarvestOutcome::Cached,
                    bytes: Some(1_048_576),
                }),
            )
            .await?;
        store
            .record_event(
                5001,
                None,
                &StatsEvent::System(SystemEvent::CacheHarvest {
                    song: song(),
                    quality: "high".to_owned(),
                    format: "unknown".to_owned(),
                    outcome: CacheHarvestOutcome::Discarded,
                    bytes: None,
                }),
            )
            .await?;

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            format: String,
            outcome: CacheHarvestOutcome,
            bytes: Option<i64>,
        }
        let rows = crate::entity::cache_harvests::Entity::find()
            .select_only()
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::cache_harvests::Column::Format,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::cache_harvests::Column::Outcome,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::cache_harvests::Column::Bytes,
            ))
            .order_by_asc(sea_orm::sea_query::Expr::col(
                crate::entity::cache_harvests::Column::Id,
            ))
            .into_model::<Row>()
            .all(live(&store)?)
            .await?;
        let first = rows
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected first row"))?;
        let second = rows
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("expected second row"))?;
        assert_eq!(first.format, "flac");
        assert_eq!(first.outcome, CacheHarvestOutcome::Cached);
        assert_eq!(first.bytes, Some(1_048_576));
        assert_eq!(second.format, "unknown");
        assert_eq!(second.outcome, CacheHarvestOutcome::Discarded);
        assert_eq!(second.bytes, None);
        Ok(())
    }
}

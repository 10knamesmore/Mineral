//! 行为域事件的落库:每张表一条编译期校验的 `query!`。

use color_eyre::eyre::WrapErr as _;
use mineral_model::SongId;
use sqlx::SqlitePool;

use crate::event::BehaviorEvent;
use crate::vocab::Actor;
use crate::{
    ActionTrigger, AudioBackend, CopyContext, DownloadHook, DownloadOutcome, FetchOutcome,
    FetchTrigger, LifecyclePhase, LifecycleWho, LoveOrigin, OpOutcome, PauseAction, PlaylistError,
    QueueOp, RejectReason, RemoteMirror, SearchOutcome, SearchTargetKind, SpawnOutcome, StoreOp,
};

/// 行为域每张表都有的公共写入列 + live 连接句柄。
struct BehaviorWrite<'a> {
    /// live 连接池。
    pool: &'a SqlitePool,

    /// 事件时刻 epoch ms。
    ts: i64,

    /// 归属会话 id;无会话上下文落 NULL。
    session_id: Option<i64>,

    /// 发起方。
    actor: Actor,
}

/// 按变体把一条行为域事件落到对应表。公共列(ts / session_id / actor)在此汇入。
pub(super) async fn write(
    pool: &SqlitePool,
    ts: i64,
    session_id: Option<i64>,
    actor: Actor,
    event: &BehaviorEvent,
) -> color_eyre::Result<()> {
    let w = BehaviorWrite {
        pool,
        ts,
        session_id,
        actor,
    };
    match event {
        BehaviorEvent::Search {
            query,
            query_hash,
            kind,
            source,
            page,
            result_count,
            outcome,
        } => {
            write_search(
                &w,
                SearchTerm {
                    raw: query.as_deref(),
                    hash: query_hash,
                },
                *kind,
                source.name(),
                *page,
                *result_count,
                *outcome,
            )
            .await
        }
        BehaviorEvent::Seek {
            song,
            from_ms,
            to_ms,
        } => write_seek(&w, song, *from_ms, *to_ms).await,
        BehaviorEvent::Pause {
            song,
            at_ms,
            action,
        } => write_pause(&w, song, *at_ms, *action).await,
        BehaviorEvent::VolumeChange { from_pct, to_pct } => {
            write_volume_change(&w, *from_pct, *to_pct).await
        }
        BehaviorEvent::ModeChange { from_mode, to_mode } => {
            write_mode_change(&w, *from_mode, *to_mode).await
        }
        BehaviorEvent::LoveChange {
            song,
            loved,
            origin,
            remote_mirror,
        } => write_love_change(&w, song, *loved, *origin, *remote_mirror).await,
        BehaviorEvent::QueueOp { op, song, count } => {
            write_queue_op(&w, *op, song.as_ref(), *count).await
        }
        BehaviorEvent::PlaylistOp {
            op,
            playlist_ref,
            song,
            song_count,
            outcome,
            error_kind,
        } => {
            write_playlist_op(
                &w,
                *op,
                playlist_ref,
                song.as_ref(),
                *song_count,
                *outcome,
                *error_kind,
            )
            .await
        }
        BehaviorEvent::Fetch {
            fetch_kind,
            source,
            target_ref,
            trigger,
            outcome,
            latency_ms,
        } => {
            write_fetch(
                &w,
                *fetch_kind,
                source.name(),
                target_ref.as_deref(),
                *trigger,
                *outcome,
                *latency_ms,
            )
            .await
        }
        BehaviorEvent::Download {
            song,
            quality,
            format,
            outcome,
            hooked,
            path,
        } => {
            write_download(
                &w,
                song,
                quality,
                format.as_deref(),
                *outcome,
                *hooked,
                path.as_deref(),
            )
            .await
        }
        BehaviorEvent::TaskCancel { filter_tags } => write_task_cancel(&w, filter_tags).await,
        BehaviorEvent::CopyRender {
            template_index,
            ctx_kind,
            target_ref,
            outcome,
        } => {
            write_copy_render(
                &w,
                *template_index,
                *ctx_kind,
                target_ref.as_deref(),
                *outcome,
            )
            .await
        }
        BehaviorEvent::ActionInvocation {
            name,
            trigger,
            outcome,
        } => write_action_invocation(&w, name, *trigger, *outcome).await,
        BehaviorEvent::ConfigOverride { path } => write_config_override(&w, path).await,
        BehaviorEvent::StoreWrite { song, key, op } => write_store_write(&w, song, key, *op).await,
        BehaviorEvent::Spawn {
            program,
            outcome,
            exit_code,
        } => write_spawn(&w, program, *outcome, *exit_code).await,
        BehaviorEvent::BusMessage { name } => write_bus_message(&w, name).await,
        BehaviorEvent::FullscreenChange { fullscreen } => {
            write_fullscreen_change(&w, *fullscreen).await
        }
        BehaviorEvent::ConnectionReject { reason } => write_connection_reject(&w, *reason).await,
        BehaviorEvent::AppLifecycle {
            who,
            phase,
            audio_backend,
            session_restored,
            client_version,
        } => {
            write_app_lifecycle(
                &w,
                *who,
                *phase,
                *audio_backend,
                *session_restored,
                client_version.as_deref(),
            )
            .await
        }
    }
}

/// 拆 `Option<SongId>` 成 `(ns, song_value)` 两列;`None` 时两列皆 NULL。
fn split_song(song: Option<&SongId>) -> (Option<&str>, Option<&str>) {
    match song {
        Some(s) => (Some(s.namespace().name()), Some(s.value())),
        None => (None, None),
    }
}

/// searches 的搜索词两种表示(受 search_queries 配置控制)。合成一个参数,避免
/// write_search 超 clippy 参数上限;raw 与 hash 语义上就是同一词的两种视图。
#[derive(Clone, Copy)]
struct SearchTerm<'a> {
    /// 原文(仅 raw 模式);hashed / off 为 None。
    raw: Option<&'a str>,

    /// 不可逆散列(恒存,去重 / 计数口径)。
    hash: &'a str,
}

/// 落 searches 一行。
async fn write_search(
    w: &BehaviorWrite<'_>,
    term: SearchTerm<'_>,
    kind: SearchTargetKind,
    source: &str,
    page: i64,
    result_count: Option<i64>,
    outcome: SearchOutcome,
) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO searches (ts, session_id, actor, query, query_hash, kind, source, page, result_count, outcome) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        term.raw,
        term.hash,
        kind as _,
        source,
        page,
        result_count,
        outcome as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event searches 落库失败")?;
    Ok(())
}

/// 落 seeks 一行。
async fn write_seek(
    w: &BehaviorWrite<'_>,
    song: &SongId,
    from_ms: i64,
    to_ms: i64,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    sqlx::query!(
        "INSERT INTO seeks (ts, session_id, actor, ns, song_value, from_ms, to_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        ns,
        song_value,
        from_ms,
        to_ms,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event seeks 落库失败")?;
    Ok(())
}

/// 落 pauses 一行。
async fn write_pause(
    w: &BehaviorWrite<'_>,
    song: &SongId,
    at_ms: i64,
    action: PauseAction,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    sqlx::query!(
        "INSERT INTO pauses (ts, session_id, actor, ns, song_value, at_ms, action) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        ns,
        song_value,
        at_ms,
        action as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event pauses 落库失败")?;
    Ok(())
}

/// 落 volume_changes 一行。
async fn write_volume_change(
    w: &BehaviorWrite<'_>,
    from_pct: i64,
    to_pct: i64,
) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO volume_changes (ts, session_id, actor, from_pct, to_pct) VALUES (?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        from_pct,
        to_pct,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event volume_changes 落库失败")?;
    Ok(())
}

/// 落 mode_changes 一行。
async fn write_mode_change(
    w: &BehaviorWrite<'_>,
    from_mode: crate::PlayMode,
    to_mode: crate::PlayMode,
) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO mode_changes (ts, session_id, actor, from_mode, to_mode) VALUES (?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        from_mode as _,
        to_mode as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event mode_changes 落库失败")?;
    Ok(())
}

/// 落 love_changes 一行。
async fn write_love_change(
    w: &BehaviorWrite<'_>,
    song: &SongId,
    loved: bool,
    origin: LoveOrigin,
    remote_mirror: Option<RemoteMirror>,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    let loved = i64::from(loved);
    sqlx::query!(
        "INSERT INTO love_changes (ts, session_id, actor, ns, song_value, loved, origin, remote_mirror) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        ns,
        song_value,
        loved,
        origin as _,
        remote_mirror as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event love_changes 落库失败")?;
    Ok(())
}

/// 落 queue_ops 一行。
async fn write_queue_op(
    w: &BehaviorWrite<'_>,
    op: QueueOp,
    song: Option<&SongId>,
    count: i64,
) -> color_eyre::Result<()> {
    let (ns, song_value) = split_song(song);
    sqlx::query!(
        "INSERT INTO queue_ops (ts, session_id, actor, op, ns, song_value, count) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        op as _,
        ns,
        song_value,
        count,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event queue_ops 落库失败")?;
    Ok(())
}

/// 落 playlist_ops 一行。
async fn write_playlist_op(
    w: &BehaviorWrite<'_>,
    op: crate::PlaylistOpKind,
    playlist_ref: &crate::PlaylistRef,
    song: Option<&SongId>,
    song_count: i64,
    outcome: OpOutcome,
    error_kind: Option<PlaylistError>,
) -> color_eyre::Result<()> {
    let (ns, song_value) = split_song(song);
    let playlist_ref = playlist_ref.to_column();
    sqlx::query!(
        "INSERT INTO playlist_ops (ts, session_id, actor, op, playlist_ref, ns, song_value, song_count, outcome, error_kind) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        op as _,
        playlist_ref,
        ns,
        song_value,
        song_count,
        outcome as _,
        error_kind as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event playlist_ops 落库失败")?;
    Ok(())
}

/// 落 fetches 一行。
async fn write_fetch(
    w: &BehaviorWrite<'_>,
    fetch_kind: crate::FetchKind,
    source: &str,
    target_ref: Option<&str>,
    trigger: FetchTrigger,
    outcome: FetchOutcome,
    latency_ms: i64,
) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO fetches (ts, session_id, actor, fetch_kind, source, target_ref, trigger, outcome, latency_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        fetch_kind as _,
        source,
        target_ref,
        trigger as _,
        outcome as _,
        latency_ms,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event fetches 落库失败")?;
    Ok(())
}

/// 落 downloads 一行。
async fn write_download(
    w: &BehaviorWrite<'_>,
    song: &SongId,
    quality: &str,
    format: Option<&str>,
    outcome: DownloadOutcome,
    hooked: DownloadHook,
    path: Option<&str>,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    sqlx::query!(
        "INSERT INTO downloads (ts, session_id, actor, ns, song_value, quality, format, outcome, hooked, path) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        ns,
        song_value,
        quality,
        format,
        outcome as _,
        hooked as _,
        path,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event downloads 落库失败")?;
    Ok(())
}

/// 落 task_cancels 一行。
async fn write_task_cancel(w: &BehaviorWrite<'_>, filter_tags: &str) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO task_cancels (ts, session_id, actor, filter_tags) VALUES (?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        filter_tags,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event task_cancels 落库失败")?;
    Ok(())
}

/// 落 copy_renders 一行。
async fn write_copy_render(
    w: &BehaviorWrite<'_>,
    template_index: i64,
    ctx_kind: CopyContext,
    target_ref: Option<&str>,
    outcome: OpOutcome,
) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO copy_renders (ts, session_id, actor, template_index, ctx_kind, target_ref, outcome) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        template_index,
        ctx_kind as _,
        target_ref,
        outcome as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event copy_renders 落库失败")?;
    Ok(())
}

/// 落 action_invocations 一行。
async fn write_action_invocation(
    w: &BehaviorWrite<'_>,
    name: &str,
    trigger: ActionTrigger,
    outcome: OpOutcome,
) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO action_invocations (ts, session_id, actor, name, trigger, outcome) \
         VALUES (?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        name,
        trigger as _,
        outcome as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event action_invocations 落库失败")?;
    Ok(())
}

/// 落 config_overrides 一行。
async fn write_config_override(w: &BehaviorWrite<'_>, path: &str) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO config_overrides (ts, session_id, actor, path) VALUES (?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        path,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event config_overrides 落库失败")?;
    Ok(())
}

/// 落 store_writes 一行。
async fn write_store_write(
    w: &BehaviorWrite<'_>,
    song: &SongId,
    key: &str,
    op: StoreOp,
) -> color_eyre::Result<()> {
    let ns = song.namespace().name();
    let song_value = song.value();
    sqlx::query!(
        "INSERT INTO store_writes (ts, session_id, actor, ns, song_value, key, op) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        ns,
        song_value,
        key,
        op as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event store_writes 落库失败")?;
    Ok(())
}

/// 落 spawns 一行。
async fn write_spawn(
    w: &BehaviorWrite<'_>,
    program: &str,
    outcome: SpawnOutcome,
    exit_code: Option<i64>,
) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO spawns (ts, session_id, actor, program, outcome, exit_code) \
         VALUES (?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        program,
        outcome as _,
        exit_code,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event spawns 落库失败")?;
    Ok(())
}

/// 落 bus_messages 一行。
async fn write_bus_message(w: &BehaviorWrite<'_>, name: &str) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO bus_messages (ts, session_id, actor, name) VALUES (?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        name,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event bus_messages 落库失败")?;
    Ok(())
}

/// 落 fullscreen_changes 一行。
async fn write_fullscreen_change(
    w: &BehaviorWrite<'_>,
    fullscreen: bool,
) -> color_eyre::Result<()> {
    let fullscreen = i64::from(fullscreen);
    sqlx::query!(
        "INSERT INTO fullscreen_changes (ts, session_id, actor, fullscreen) VALUES (?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        fullscreen,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event fullscreen_changes 落库失败")?;
    Ok(())
}

/// 落 connection_rejects 一行。
async fn write_connection_reject(
    w: &BehaviorWrite<'_>,
    reason: RejectReason,
) -> color_eyre::Result<()> {
    sqlx::query!(
        "INSERT INTO connection_rejects (ts, session_id, actor, reason) VALUES (?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        reason as _,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event connection_rejects 落库失败")?;
    Ok(())
}

/// 落 app_lifecycle 一行。
async fn write_app_lifecycle(
    w: &BehaviorWrite<'_>,
    who: LifecycleWho,
    phase: LifecyclePhase,
    audio_backend: Option<AudioBackend>,
    session_restored: Option<bool>,
    client_version: Option<&str>,
) -> color_eyre::Result<()> {
    let session_restored = session_restored.map(i64::from);
    sqlx::query!(
        "INSERT INTO app_lifecycle (ts, session_id, actor, who, phase, audio_backend, session_restored, client_version) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        w.ts,
        w.session_id,
        w.actor as _,
        who as _,
        phase as _,
        audio_backend as _,
        session_restored,
        client_version,
    )
    .execute(w.pool)
    .await
    .wrap_err("record_event app_lifecycle 落库失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::event::{
        BehaviorEvent, LoveOrigin, OpOutcome, PlaylistError, QueueOp, SearchOutcome,
        SearchTargetKind, StatsEvent,
    };
    use crate::store::StatsStore;
    use crate::vocab::Actor;
    use mineral_model::{SongId, SourceKind};
    use sqlx::SqlitePool;

    /// 建落盘临时库并开一个会话,返回目录守卫、句柄与会话 id。
    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore, i64)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        let sid = store
            .open_session(1000)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected session id"))?;
        Ok((dir, store, sid))
    }

    /// 从句柄取 live pool。
    fn live(store: &StatsStore) -> color_eyre::Result<&SqlitePool> {
        store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected live pool"))
    }

    /// searches:nullable query / result_count 落 NULL、outcome=failed、actor 汇入。
    #[tokio::test]
    async fn record_search_nullable_and_outcome() -> color_eyre::Result<()> {
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::Cli,
            event: BehaviorEvent::Search {
                query: None,
                query_hash: "h1".to_owned(),
                kind: SearchTargetKind::Album,
                source: SourceKind::NETEASE,
                page: 2,
                result_count: None,
                outcome: SearchOutcome::Failed,
            },
        };
        store.record_event(2000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            query: Option<String>,
            kind: SearchTargetKind,
            page: i64,
            result_count: Option<i64>,
            outcome: SearchOutcome,
            actor: Actor,
            session_id: Option<i64>,
        }
        let row = sqlx::query_as::<_, Row>(
            "SELECT query, kind, page, result_count, outcome, actor, session_id FROM searches",
        )
        .fetch_one(live(&store)?)
        .await?;
        assert_eq!(row.query, None);
        assert_eq!(row.kind, SearchTargetKind::Album);
        assert_eq!(row.page, 2);
        assert_eq!(row.result_count, None);
        assert_eq!(row.outcome, SearchOutcome::Failed);
        assert_eq!(row.actor, Actor::Cli);
        assert_eq!(row.session_id, Some(sid));
        Ok(())
    }

    /// love_changes:origin=import 旁路、remote_mirror 落 NULL、bool 存 1、song 拆两列。
    #[tokio::test]
    async fn record_love_change_import_no_mirror() -> color_eyre::Result<()> {
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::User,
            event: BehaviorEvent::LoveChange {
                song: SongId::new(SourceKind::NETEASE, "42"),
                loved: true,
                origin: LoveOrigin::Import,
                remote_mirror: None,
            },
        };
        store.record_event(3000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            ns: String,
            song_value: String,
            loved: i64,
            origin: LoveOrigin,
            remote_mirror: Option<String>,
        }
        let row = sqlx::query_as::<_, Row>(
            "SELECT ns, song_value, loved, origin, remote_mirror FROM love_changes",
        )
        .fetch_one(live(&store)?)
        .await?;
        assert_eq!(row.ns, "netease");
        assert_eq!(row.song_value, "42");
        assert_eq!(row.loved, 1, "bool true 存 1");
        assert_eq!(row.origin, LoveOrigin::Import);
        assert_eq!(row.remote_mirror, None);
        Ok(())
    }

    /// playlist_ops:outcome=failed + error_kind=auth_required,自由文本 op 原样落。
    #[tokio::test]
    async fn record_playlist_op_failed_with_error() -> color_eyre::Result<()> {
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::User,
            event: BehaviorEvent::PlaylistOp {
                op: crate::PlaylistOpKind::Add,
                playlist_ref: crate::PlaylistRef::Existing(mineral_model::PlaylistId::new(
                    SourceKind::NETEASE,
                    "7",
                )),
                song: Some(SongId::new(SourceKind::NETEASE, "42")),
                song_count: 1,
                outcome: OpOutcome::Failed,
                error_kind: Some(PlaylistError::AuthRequired),
            },
        };
        store.record_event(4000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            op: String,
            playlist_ref: String,
            song_value: Option<String>,
            song_count: i64,
            outcome: OpOutcome,
            error_kind: Option<PlaylistError>,
        }
        let row = sqlx::query_as::<_, Row>(
            "SELECT op, playlist_ref, song_value, song_count, outcome, error_kind FROM playlist_ops",
        )
        .fetch_one(live(&store)?)
        .await?;
        assert_eq!(row.op, "add");
        assert_eq!(row.playlist_ref, "netease:7");
        assert_eq!(row.song_value, Some("42".to_owned()));
        assert_eq!(row.song_count, 1);
        assert_eq!(row.outcome, OpOutcome::Failed);
        assert_eq!(row.error_kind, Some(PlaylistError::AuthRequired));
        Ok(())
    }

    /// queue_ops:op=set + 无单曲(ns / song_value 落 NULL)、count 落值。
    #[tokio::test]
    async fn record_queue_op_set_no_song() -> color_eyre::Result<()> {
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::User,
            event: BehaviorEvent::QueueOp {
                op: QueueOp::Set,
                song: None,
                count: 5,
            },
        };
        store.record_event(5000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            op: QueueOp,
            ns: Option<String>,
            song_value: Option<String>,
            count: i64,
        }
        let row = sqlx::query_as::<_, Row>("SELECT op, ns, song_value, count FROM queue_ops")
            .fetch_one(live(&store)?)
            .await?;
        assert_eq!(row.op, QueueOp::Set);
        assert_eq!(row.ns, None);
        assert_eq!(row.song_value, None);
        assert_eq!(row.count, 5);
        Ok(())
    }

    /// connection_rejects:reason 落值(过 CHECK 约束)、actor=system(daemon 主动拒)。
    #[tokio::test]
    async fn record_connection_reject_version_mismatch() -> color_eyre::Result<()> {
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::System,
            event: BehaviorEvent::ConnectionReject {
                reason: crate::event::RejectReason::VersionMismatch,
            },
        };
        store.record_event(6000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            reason: crate::event::RejectReason,
            actor: Actor,
        }
        let row = sqlx::query_as::<_, Row>("SELECT reason, actor FROM connection_rejects")
            .fetch_one(live(&store)?)
            .await?;
        assert_eq!(row.reason, crate::event::RejectReason::VersionMismatch);
        assert_eq!(row.actor, Actor::System);
        Ok(())
    }

    /// copy_renders:template_index / ctx_kind / target_ref / outcome 各列落值。
    #[tokio::test]
    async fn record_copy_render_playlist_failed() -> color_eyre::Result<()> {
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::User,
            event: BehaviorEvent::CopyRender {
                template_index: 3,
                ctx_kind: crate::event::CopyContext::Playlist,
                target_ref: Some("netease:99".to_owned()),
                outcome: OpOutcome::Failed,
            },
        };
        store.record_event(7000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            template_index: i64,
            ctx_kind: crate::event::CopyContext,
            target_ref: Option<String>,
            outcome: OpOutcome,
        }
        let row = sqlx::query_as::<_, Row>(
            "SELECT template_index, ctx_kind, target_ref, outcome FROM copy_renders",
        )
        .fetch_one(live(&store)?)
        .await?;
        assert_eq!(row.template_index, 3);
        assert_eq!(row.ctx_kind, crate::event::CopyContext::Playlist);
        assert_eq!(row.target_ref, Some("netease:99".to_owned()));
        assert_eq!(row.outcome, OpOutcome::Failed);
        Ok(())
    }

    /// spawns:program / outcome / exit_code 落值,actor=script。
    #[tokio::test]
    async fn record_spawn_exited_with_code() -> color_eyre::Result<()> {
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::Script,
            event: BehaviorEvent::Spawn {
                program: "notify-send".to_owned(),
                outcome: crate::event::SpawnOutcome::Exited,
                exit_code: Some(0),
            },
        };
        store.record_event(8000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            program: String,
            outcome: crate::event::SpawnOutcome,
            exit_code: Option<i64>,
            actor: Actor,
        }
        let row = sqlx::query_as::<_, Row>("SELECT program, outcome, exit_code, actor FROM spawns")
            .fetch_one(live(&store)?)
            .await?;
        assert_eq!(row.program, "notify-send");
        assert_eq!(row.outcome, crate::event::SpawnOutcome::Exited);
        assert_eq!(row.exit_code, Some(0));
        assert_eq!(row.actor, Actor::Script);
        Ok(())
    }

    /// bus_messages:name 落值、actor=script(脚本 mineral.emit)。
    #[tokio::test]
    async fn record_bus_message_from_script() -> color_eyre::Result<()> {
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::Script,
            event: BehaviorEvent::BusMessage {
                name: "myplugin.tick".to_owned(),
            },
        };
        store.record_event(9000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            name: String,
            actor: Actor,
        }
        let row = sqlx::query_as::<_, Row>("SELECT name, actor FROM bus_messages")
            .fetch_one(live(&store)?)
            .await?;
        assert_eq!(row.name, "myplugin.tick");
        assert_eq!(row.actor, Actor::Script);
        Ok(())
    }

    /// fetches:fetch_kind / target_ref / trigger / outcome / latency_ms 各列;system 触发。
    #[tokio::test]
    async fn record_fetch_system_trigger() -> color_eyre::Result<()> {
        use crate::event::{FetchOutcome, FetchTrigger};
        let (_dir, store, sid) = open_temp().await?;
        let event = StatsEvent::Behavior {
            actor: Actor::System,
            event: BehaviorEvent::Fetch {
                fetch_kind: crate::FetchKind::PlaylistDetail,
                source: SourceKind::NETEASE,
                target_ref: Some("netease:7".to_owned()),
                trigger: FetchTrigger::System,
                outcome: FetchOutcome::Ok,
                latency_ms: 123,
            },
        };
        store.record_event(10_000, Some(sid), &event).await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            fetch_kind: String,
            target_ref: Option<String>,
            trigger: FetchTrigger,
            outcome: FetchOutcome,
            latency_ms: i64,
        }
        let row = sqlx::query_as::<_, Row>(
            "SELECT fetch_kind, target_ref, trigger, outcome, latency_ms FROM fetches",
        )
        .fetch_one(live(&store)?)
        .await?;
        assert_eq!(row.fetch_kind, "playlist_detail");
        assert_eq!(row.target_ref, Some("netease:7".to_owned()));
        assert_eq!(row.trigger, FetchTrigger::System);
        assert_eq!(row.outcome, FetchOutcome::Ok);
        assert_eq!(row.latency_ms, 123);
        Ok(())
    }
}

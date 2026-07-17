//! 确定性种子生成器:压测与聚合正确性测试共用的可调量级造数。
//!
//! 全部取值由行下标 `i` 算出(不碰随机数 / 系统时钟),故同 `(plays, events)` 参数跨
//! 运行、跨机器恒等——压测可复现、正确性断言可写死。时间戳在一年窗口内单调铺开,喂
//! 时间分桶与会话切分;播放跨 3 来源 / 6 上下文 / 多格式 / 4 结束原因铺开,事件轮转
//! 覆盖行为域 + 系统域的代表表,喂各维聚合与库体积。
//!
//! 只在 `test` cfg 或 `fixture` feature 下编译,不进发布 API。

use mineral_model::{AlbumId, ArtistId, AudioFormat, BitRate, PlaylistId, SongId, SourceKind};

use crate::{
    Actor, BehaviorEvent, CacheHarvestOutcome, FetchOutcome, FetchTrigger, FinishReason,
    GaplessResult, HookDecision, HookKind, HookStage, LoveOrigin, PauseAction, PlayOrigin,
    PlayRecord, PlaybackOrigin, PrefetchResolution, PrefetchSource, QueueContext, QueueOp,
    RemoteMirror, SearchOutcome, SearchTargetKind, StatsEvent, StatsStore, SystemEvent, UrlOutcome,
    query_hash,
};

/// 一年窗口毫秒跨度(种子时间戳落在 `[0, YEAR_MS)`,单调铺开)。
const YEAR_MS: i64 = 365 * 86_400_000;

/// 不同歌曲数(播放行按 `i % SONG_POOL` 复用,喂 top_songs 分组聚合)。
const SONG_POOL: i64 = 400;

/// 不同实体上下文引用数(playlist / album / artist 按 `i % CONTEXT_POOL` 复用,喂
/// top_contexts 分组)。
const CONTEXT_POOL: i64 = 50;

/// 每多少行播放切一个新会话(粗粒度,喂 sessions 聚合 / endurance)。
const PLAYS_PER_SESSION: i64 = 40;

/// 第 `i` 项落在哪个来源(裸 name;三桶轮转)。
fn source_name(i: i64) -> &'static str {
    match i % 3 {
        0 => "netease",
        1 => "bilibili",
        _ => "local",
    }
}

/// 第 `i` 项的来源身份。
fn source(i: i64) -> SourceKind {
    SourceKind::from_name(source_name(i))
}

/// 第 `i` 项归属的歌曲(池内复用,来源随歌固定)。
fn song(i: i64) -> SongId {
    let n = i.rem_euclid(SONG_POOL);
    SongId::new(source(n), n.to_string())
}

/// 第 `i` 项的发起方(多数用户,少量脚本 / 系统 / CLI)。
fn actor(i: i64) -> Actor {
    match i % 8 {
        0 => Actor::Script,
        1 => Actor::System,
        2 => Actor::Cli,
        _ => Actor::User,
    }
}

/// 第 `i` 行播放的结束原因(多数自然播完,掺跳过 / 停止 / 错误)。
fn finish(i: i64) -> FinishReason {
    match i % 10 {
        0 | 1 => FinishReason::Skip,
        2 => FinishReason::Stop,
        3 => FinishReason::Error,
        _ => FinishReason::Eof,
    }
}

/// 第 `i` 行播放的模式串。
fn mode(i: i64) -> crate::PlayMode {
    match i % 3 {
        0 => crate::PlayMode::Sequential,
        1 => crate::PlayMode::Shuffle,
        _ => crate::PlayMode::RepeatOne,
    }
}

/// 第 `i` 行播放的发起方式。
fn origin(i: i64) -> PlayOrigin {
    match i % 5 {
        0 => PlayOrigin::Explicit,
        1 => PlayOrigin::AutoAdvance,
        2 => PlayOrigin::Resume,
        3 => PlayOrigin::Script,
        _ => PlayOrigin::Unknown,
    }
}

/// 第 `i` 行播放的队列上下文(六类轮转,实体引用池内复用)。
fn context(i: i64) -> QueueContext {
    let n = i.rem_euclid(CONTEXT_POOL).to_string();
    match i % 6 {
        0 => QueueContext::Search {
            query: Some(format!("ctx{}", i.rem_euclid(CONTEXT_POOL))),
        },
        1 => QueueContext::Playlist {
            id: PlaylistId::new(source(i), n),
        },
        2 => QueueContext::Album {
            id: AlbumId::new(source(i), n),
        },
        3 => QueueContext::Artist {
            id: ArtistId::new(source(i), n),
        },
        4 => QueueContext::Manual,
        _ => QueueContext::Unknown,
    }
}

/// 第 `i` 行播放的音频格式(掺一档 `None` 未知)。
fn format(i: i64) -> Option<AudioFormat> {
    match i % 5 {
        0 => Some(AudioFormat::Flac),
        1 => Some(AudioFormat::Mp3),
        2 => Some(AudioFormat::Aac),
        3 => Some(AudioFormat::Wav),
        _ => None,
    }
}

/// 第 `i` 行播放的归一化音质档。
fn quality(i: i64) -> BitRate {
    match i % 5 {
        0 => BitRate::Standard,
        1 => BitRate::Higher,
        2 => BitRate::Exhigh,
        3 => BitRate::Lossless,
        _ => BitRate::Hires,
    }
}

/// 第 `i` 行播放的音频本体来源位置。
fn playback(i: i64) -> PlaybackOrigin {
    match i % 3 {
        0 => PlaybackOrigin::Remote,
        1 => PlaybackOrigin::Cache,
        _ => PlaybackOrigin::Download,
    }
}

/// 造第 `i` 行播放事实(挂到会话 `session_id`,起播时刻 `started_at`)。
///
/// 跳过行的收听时长 = 跳歌位置(短);其余行按下标散在 30s–240s。列值全由 `i` 派生,
/// 确定性。
///
/// # Params:
///   - `i`: 行下标(驱动全部列取值)
///   - `session_id`: 归属会话 id
///   - `started_at`: 起播时刻 epoch ms
///
/// # Return:
///   一行完整播放事实
pub fn play_record(i: i64, session_id: i64, started_at: i64) -> PlayRecord {
    let reason = finish(i);
    let (listen_ms, skip_at_ms) = if matches!(reason, FinishReason::Skip) {
        let at = 5_000 + i.rem_euclid(60_000);
        (at, Some(at))
    } else {
        (30_000 + i.rem_euclid(210_000), None)
    };
    PlayRecord {
        song_id: song(i),
        started_at,
        ended_at: started_at.saturating_add(listen_ms),
        listen_ms,
        duration_ms_snapshot: Some(180_000 + i.rem_euclid(120_000)),
        finish_reason: reason,
        skip_at_ms,
        play_mode: mode(i),
        session_id,
        origin: origin(i),
        actor: actor(i),
        context: context(i),
        audio: crate::PlayAudioSnapshot {
            audio_format: format(i),
            bitrate_bps: Some(128_000 + i.rem_euclid(900_000)),
            quality: Some(quality(i)),
            bit_depth: Some(if i % 2 == 0 { 16 } else { 24 }),
            substituted: i % 17 == 0,
        },
        playback_origin: playback(i),
    }
}

/// 包一层行为域事件(带 `i` 派生的 actor)。
fn behavior(i: i64, event: BehaviorEvent) -> StatsEvent {
    StatsEvent::Behavior {
        actor: actor(i),
        event,
    }
}

/// 造第 `i` 条交互事件(12 类轮转,横跨行为域 7 表 + 系统域 5 表)。
///
/// 覆盖的表:searches / seeks / pauses / volume_changes / love_changes / queue_ops /
/// fetches(行为域)+ url_resolutions / hook_fires / prefetches / cache_harvests /
/// gapless_boundaries(系统域)。列值全由 `i` 派生,确定性。
///
/// # Params:
///   - `i`: 行下标(选变体 + 驱动全部列取值)
///
/// # Return:
///   一条待落库事件
pub fn event(i: i64) -> StatsEvent {
    let song = song(i);
    match i % 12 {
        0 => behavior(
            i,
            BehaviorEvent::Search {
                query: Some(format!("q{}", i.rem_euclid(200))),
                query_hash: query_hash(&format!("q{}", i.rem_euclid(200))),
                kind: SearchTargetKind::Song,
                source: source(i),
                page: i % 4,
                result_count: Some(i.rem_euclid(60)),
                outcome: SearchOutcome::Ok,
            },
        ),
        1 => behavior(
            i,
            BehaviorEvent::Seek {
                song,
                from_ms: (i * 900).rem_euclid(200_000),
                to_ms: (i * 1_700).rem_euclid(200_000),
            },
        ),
        2 => behavior(
            i,
            BehaviorEvent::Pause {
                song,
                at_ms: (i * 1_300).rem_euclid(200_000),
                action: if i % 2 == 0 {
                    PauseAction::Pause
                } else {
                    PauseAction::Resume
                },
            },
        ),
        3 => behavior(
            i,
            BehaviorEvent::VolumeChange {
                from_pct: i.rem_euclid(100),
                to_pct: (i * 7).rem_euclid(100),
            },
        ),
        4 => behavior(
            i,
            BehaviorEvent::LoveChange {
                song,
                loved: i % 2 == 0,
                origin: LoveOrigin::User,
                remote_mirror: Some(RemoteMirror::Ok),
            },
        ),
        5 => behavior(
            i,
            BehaviorEvent::QueueOp {
                op: QueueOp::Append,
                song: Some(song),
                count: 1 + i.rem_euclid(20),
            },
        ),
        6 => behavior(
            i,
            BehaviorEvent::Fetch {
                fetch_kind: crate::FetchKind::PlaylistDetail,
                source: source(i),
                target_ref: Some(i.rem_euclid(CONTEXT_POOL).to_string()),
                trigger: if i % 2 == 0 {
                    FetchTrigger::User
                } else {
                    FetchTrigger::System
                },
                outcome: FetchOutcome::Ok,
                latency_ms: 20 + i.rem_euclid(400),
            },
        ),
        7 => StatsEvent::System(SystemEvent::UrlResolution {
            song,
            quality_requested: "lossless".to_owned(),
            outcome: UrlOutcome::Ok,
            for_prefetch: i % 3 == 0,
        }),
        8 => StatsEvent::System(SystemEvent::HookFire {
            song: Some(song),
            hook: HookKind::BeforeStream,
            stage: if i % 2 == 0 {
                HookStage::Immediate
            } else {
                HookStage::Prefetch
            },
            decision: HookDecision::Continue,
            fail_open: None,
        }),
        9 => StatsEvent::System(SystemEvent::Prefetch {
            song,
            source: PrefetchSource::Remote,
            resolution: PrefetchResolution::Armed,
        }),
        10 => StatsEvent::System(SystemEvent::CacheHarvest {
            song,
            quality: "lossless".to_owned(),
            format: "flac".to_owned(),
            outcome: CacheHarvestOutcome::Cached,
            bytes: Some(1_000_000 + i.rem_euclid(9_000_000)),
        }),
        _ => StatsEvent::System(SystemEvent::GaplessBoundary {
            song,
            result: if i % 2 == 0 {
                GaplessResult::Adopt
            } else {
                GaplessResult::Fallback
            },
        }),
    }
}

/// 事件轮转挂到已建会话之一;无会话(plays=0)则挂 NULL。
fn pick_session(sessions: &[i64], i: i64) -> Option<i64> {
    let len = i64::try_from(sessions.len()).unwrap_or(0);
    if len == 0 {
        return None;
    }
    let idx = usize::try_from(i.rem_euclid(len)).unwrap_or(0);
    sessions.get(idx).copied()
}

/// 向 `store` 灌 `plays` 行播放事实 + `events` 条交互事件(确定性、单调时间)。
///
/// 会话按 [`PLAYS_PER_SESSION`] 粗粒度切;事件轮转挂到已建会话上。`disabled` 句柄各写
/// no-op、种不进数据。
///
/// # Params:
///   - `store`: 目标 stats.db 句柄
///   - `plays`: 播放行数
///   - `events`: 事件条数
pub async fn seed(store: &StatsStore, plays: i64, events: i64) -> color_eyre::Result<()> {
    let mut sessions = Vec::<i64>::new();
    let play_step = (YEAR_MS / plays.max(1)).max(1);
    for i in 0..plays {
        let started_at = i.saturating_mul(play_step);
        if i % PLAYS_PER_SESSION == 0
            && let Some(id) = store.open_session(started_at).await?
        {
            sessions.push(id);
        }
        let session_id = sessions.last().copied().unwrap_or(1);
        store
            .record_play(&play_record(i, session_id, started_at))
            .await?;
        store
            .touch_session(session_id, started_at.saturating_add(60_000))
            .await?;
    }
    let event_step = (YEAR_MS / events.max(1)).max(1);
    for i in 0..events {
        let ts = i.saturating_mul(event_step);
        store
            .record_event(ts, pick_session(&sessions, i), &event(i))
            .await?;
    }
    Ok(())
}

/// 年量级默认档(~10⁴ 播放 + 10⁵ 事件):spec §9.1 查询压测 / 库体积的种子规模。
///
/// # Params:
///   - `store`: 目标 stats.db 句柄
pub async fn seed_year(store: &StatsStore) -> color_eyre::Result<()> {
    seed(store, 10_000, 100_000).await
}

#[cfg(test)]
mod tests {
    use super::seed;
    use crate::store::StatsStore;

    async fn temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    /// 同 `(plays, events)` 参数种两库,聚合逐项恒等——确定性是压测可复现的地基。
    #[tokio::test]
    async fn seed_is_deterministic() -> color_eyre::Result<()> {
        let (_a, sa) = temp().await?;
        let (_b, sb) = temp().await?;
        seed(&sa, 120, 480).await?;
        seed(&sb, 120, 480).await?;
        let (ta, tb) = (sa.totals(0..i64::MAX).await?, sb.totals(0..i64::MAX).await?);
        assert_eq!(ta.plays, tb.plays, "播放数确定");
        assert_eq!(ta.listen_ms, tb.listen_ms, "总收听确定");
        assert_eq!(ta.completed, tb.completed, "播完数确定");
        assert_eq!(
            sa.status().await?.events,
            sb.status().await?.events,
            "事件数确定"
        );
        Ok(())
    }

    /// 参数如实落库:plays / events 计数 = 传入值(可调量级)。
    #[tokio::test]
    async fn seed_counts_match_arguments() -> color_eyre::Result<()> {
        let (_dir, store) = temp().await?;
        seed(&store, 120, 480).await?;
        assert_eq!(store.totals(0..i64::MAX).await?.plays, 120, "播放数 = 参数");
        assert_eq!(store.status().await?.events, 480, "事件数 = 参数");
        Ok(())
    }

    /// 事件横跨行为域 + 系统域多张表(轮转覆盖,非单表堆积)。
    #[tokio::test]
    async fn seed_spreads_events_across_tables() -> color_eyre::Result<()> {
        let (_dir, store) = temp().await?;
        seed(&store, 0, 120).await?;
        let summary = store.event_summary(0..i64::MAX, 100).await?;
        let nonzero = summary.table_counts.iter().filter(|e| e.count > 0).count();
        assert!(
            nonzero >= 10,
            "12 类轮转应铺开到 ≥10 张事件表,实得 {nonzero}"
        );
        Ok(())
    }
}

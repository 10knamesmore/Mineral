//! 埋点 recorder:单 writer actor(bounded mpsc + 常驻写库任务)。
//!
//! 热路径只做 params gating + 组命令 + `try_send`(不 await、无锁 I/O);写库、会话
//! 推进、在播 pending 结算都在常驻 actor 里串行,状态不需锁。config 热更经
//! [`StatsRecorder::set_params`] 整体替换,与 actor 共享同一 `ArcSwap`(热路径 load 无锁)。
//!
//! retention 每日巡检经 [`run`] 的 `select!` 分支走同一 actor(唯一 writer 不变量)。
//!
//! TODO(优化,非正确性):机会性批量事务(recv 后 try_recv 榨干、包一次
//! BEGIN..COMMIT 摊薄 fsync)—— 单写已保正确,批量仅摊薄突发 fsync。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use mineral_log::chain;
use mineral_model::{SongId, SourceKind};
use mineral_protocol::PlaybackOrigin as WirePlaybackOrigin;
use mineral_stats::{
    Actor, AudioBackend, BehaviorEvent, FinishReason, LifecyclePhase, LifecycleWho,
    PlayAudioSnapshot, PlayOrigin, PlayRecord, PlaybackOrigin, QueueContext, Retention,
    SearchOutcome, SearchQueryMode, SearchTargetKind, SessionDecision, SessionTracker, StatsEvent,
    StatsParams, StatsStore, query_hash,
};

/// 一天的秒数(retention 巡检间隔)。
const SECONDS_PER_DAY: u64 = 86_400;

/// 一天的毫秒数(retention 水位折算)。
const MS_PER_DAY: i64 = 86_400_000;

/// bounded 通道容量:人肉交互速率打不满,纯故障域隔离兜底(满则丢弃 + warn,背压绝不
/// 传导回播放路径)。
const CHANNEL_CAPACITY: usize = 4096;

/// 起播时的语境快照;结束时补齐成 [`PlayRecord`] 落库。
///
/// 除 `ended_at` / `listen_ms` / `finish_reason` / `skip_at_ms`(结束时给)与
/// `session_id`(actor 分配)外,其余 = plays 行的起播快照。
pub struct PendingPlay {
    /// 歌曲 id。
    pub song_id: SongId,

    /// 起播时刻 epoch ms。
    pub started_at: i64,

    /// 本行发起方式。
    pub origin: PlayOrigin,

    /// 发起方。
    pub actor: Actor,

    /// 队列上下文。
    pub context: QueueContext,

    /// 播放模式。
    pub play_mode: mineral_stats::PlayMode,

    /// 总长快照 ms;探不出为 `None`。
    pub duration_ms_snapshot: Option<i64>,

    /// 音频快照(起播时未知,play_url 就绪后经 enrich 整组补)。
    pub audio: PlayAudioSnapshot,

    /// 音频本体来源位置。
    pub playback_origin: PlaybackOrigin,
}

/// actor 输入。时间戳由调用方在 send 前打(事件时刻 ≠ 落库时刻)。
enum StatsCommand {
    /// 起播语境快照。
    PlayStarted(Box<PendingPlay>),

    /// 播放结束,结算在播行。
    PlayEnded {
        /// 结束时刻 epoch ms。
        ended_at: i64,

        /// 实际收听 ms。
        listen_ms: i64,

        /// 结束原因。
        finish_reason: FinishReason,

        /// 跳歌位置 ms(仅 skip)。
        skip_at_ms: Option<i64>,
    },

    /// 全谱交互事件。
    Event {
        /// 事件时刻 epoch ms。
        ts: i64,

        /// 事件本体。
        event: Box<StatsEvent>,
    },

    /// 富化在播行的音频快照(play_url 就绪后整组补;脚本改写后再补一次、`substituted`
    /// 随快照带 true)。pending 缺席(起播被 gate / 已结算)则丢弃。
    EnrichAudio(PlayAudioSnapshot),

    /// 排空栅栏:FIFO 保证之前的命令均已落库,回 ack。停机前 flush 用。
    Flush {
        /// 排空完成的应答端。
        ack: oneshot::Sender<()>,
    },

    /// 优雅停机:actor 把在播 pending 按 `finish_reason=stop` 结算后退出(sender 全 drop
    /// 同义,rx 关也走同一结算)。
    Shutdown,
}

/// 当前 Unix epoch 毫秒;系统时钟早于 1970(RTC 异常)拿不出诚实时间戳,返回 `None`
/// (调用方本轮不记,不用哨兵值冒充);溢出钳 i64::MAX(纯理论上限)。
pub fn now_ms() -> Option<i64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// 全关采集参数(供 [`StatsRecorder::disabled`]:gating 一律短路)。
fn off_params() -> StatsParams {
    StatsParams::builder()
        .level(mineral_stats::Level::Off)
        .collect(rustc_hash::FxHashMap::default())
        .search_queries(mineral_stats::SearchQueryMode::Off)
        .exclude_sources(rustc_hash::FxHashSet::default())
        .gap_ms(0)
        .retention(mineral_stats::Retention::Forever)
        .build()
}

/// client 播放模式 → 埋点词汇(stats 不依赖 protocol,两枚举独立;这是唯一转换点)。
///
/// # Params:
///   - `mode`: client 侧播放模式
///
/// # Return:
///   埋点侧播放模式
pub fn stats_play_mode(mode: mineral_protocol::PlayMode) -> mineral_stats::PlayMode {
    match mode {
        mineral_protocol::PlayMode::Sequential => mineral_stats::PlayMode::Sequential,
        mineral_protocol::PlayMode::Shuffle => mineral_stats::PlayMode::Shuffle,
        mineral_protocol::PlayMode::RepeatAll => mineral_stats::PlayMode::RepeatAll,
        mineral_protocol::PlayMode::RepeatOne => mineral_stats::PlayMode::RepeatOne,
    }
}

/// 由起播现场构造起播快照:`playback_origin` 从 wire 映射;`play_origin` 与 `actor`
/// 都由点播调用点穿透——**actor 不从 origin 派生**:next / prev 的 origin 是
/// AutoAdvance(推进方式),发起者却可能是用户按键 / 脚本 / EOF 自治,两维独立;
/// `context` 由队列语境继承(client 建队列时告知),format 等 play_url 快照后续经
/// enrich 补。
///
/// # Params:
///   - `song_id`: 歌曲 id
///   - `play_mode`: 当前播放模式
///   - `duration_ms_snapshot`: 歌曲总长 ms
///   - `playback_origin`: 音频本体来源位置(wire 枚举)
///   - `play_origin`: 起播来源(显式 / 自动接续 / 脚本 / 会话恢复)
///   - `actor`: 发起方(用户按键 / 脚本 / daemon 自治)
///   - `context`: 队列语境(该曲所在队列来自搜索 / 歌单 / 专辑 / 艺人 / 手动)
///
/// # Return:
///   起播快照;系统时钟异常拿不出起播时刻时 `None`(本次播放不记)
pub fn pending_from_start(
    song_id: SongId,
    play_mode: mineral_stats::PlayMode,
    duration_ms_snapshot: Option<i64>,
    playback_origin: WirePlaybackOrigin,
    play_origin: PlayOrigin,
    actor: Actor,
    context: QueueContext,
) -> Option<PendingPlay> {
    Some(PendingPlay {
        song_id,
        started_at: now_ms()?,
        origin: play_origin,
        actor,
        context,
        play_mode,
        duration_ms_snapshot,
        audio: PlayAudioSnapshot::default(),
        playback_origin: match playback_origin {
            WirePlaybackOrigin::Download => PlaybackOrigin::Download,
            WirePlaybackOrigin::Cache => PlaybackOrigin::Cache,
            WirePlaybackOrigin::Remote => PlaybackOrigin::Remote,
        },
    })
}

/// 打点入口句柄(Clone 廉价,分发给各挂接点)。
#[derive(Clone)]
pub struct StatsRecorder {
    /// level / gap / exclude 等,配置热更整体替换;与 actor 共享(热路径 `load` 无锁)。
    params: Arc<ArcSwap<StatsParams>>,

    /// 到 writer actor 的命令通道(bounded);降级 no-op 时为 `None`。
    tx: Option<mpsc::Sender<StatsCommand>>,

    /// stats.db 只读查询用的句柄(与 actor 写同一库,Clone 指同一连接池)。
    store: StatsStore,

    /// 通道满 / 已关累计丢弃的命令数(故障域可观测;背压绝不回灌播放路径)。
    dropped: Arc<AtomicU64>,
}

impl StatsRecorder {
    /// 起 recorder:建 bounded 通道 + spawn 常驻写库 actor。
    ///
    /// # Params:
    ///   - `store`: stats.db 句柄(降级时各写 no-op)
    ///   - `params`: 初始采集参数
    ///
    /// # Return:
    ///   (句柄, actor 的 `JoinHandle`);发 [`Self::shutdown`] 或丢弃全部句柄即优雅停机
    ///   (actor 结算在播 pending 为 stop 后排空退出)
    pub fn spawn(store: StatsStore, params: StatsParams) -> (Self, JoinHandle<()>) {
        let params = Arc::new(ArcSwap::from_pointee(params));
        let (tx, rx) = mpsc::channel::<StatsCommand>(CHANNEL_CAPACITY);
        let handle = tokio::spawn(run(rx, store.clone(), Arc::clone(&params)));
        (
            Self {
                params,
                tx: Some(tx),
                store,
                dropped: Arc::new(AtomicU64::new(0)),
            },
            handle,
        )
    }

    /// 降级 no-op 句柄(无 actor / 无写库):所有打点静默丢弃、查询返回空。供无持久化的
    /// 路径(in-proc TUI、测试)构造 [`PlayerCore`] 用。
    pub fn disabled() -> Self {
        Self {
            params: Arc::new(ArcSwap::from_pointee(off_params())),
            tx: None,
            store: StatsStore::disabled(),
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// stats.db 只读查询句柄(降级时为 disabled store,各查询返回空)。
    pub fn store(&self) -> &StatsStore {
        &self.store
    }

    /// 通道满 / 已关累计丢弃的命令总数(埋点故障域可观测;测试与压测断言用)。
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 配置热更:整体替换采集参数(`session_gap` 变更只影响后续判定)。热路径无锁读。
    pub fn set_params(&self, params: StatsParams) {
        self.params.store(Arc::new(params));
    }

    /// 优雅停机:发 [`StatsCommand::Shutdown`] 让 actor 结算在播 pending 为 stop 后退出。
    /// 降级(无 actor)即刻返回;actor 已退时发送失败也无碍。
    pub async fn shutdown(&self) {
        let Some(tx) = &self.tx else {
            return;
        };
        let _ = tx.send(StatsCommand::Shutdown).await;
    }

    /// 起播:gating 通过则暂存语境快照(档位放行 + 来源未排除)。搜索语境的查询词在此
    /// 按 `search_queries` 档处理——plays.context_ref 与 searches 表同受该档约束,搜索
    /// 词不能经起播语境旁路落库。
    pub fn play_started(&self, mut pending: PendingPlay) {
        {
            let params = self.params.load();
            if !params.records_plays() || params.excludes_source(pending.song_id.namespace().name())
            {
                return;
            }
            pending.context = pending.context.redact_search(params.search_queries());
        }
        self.send(StatsCommand::PlayStarted(Box::new(pending)));
    }

    /// 播放结束:按结束原因结算在播行(起播被 gate 掉时 actor 无 pending、自动忽略)。
    /// 结束时刻内部 stamp;skip 的跳歌位置 = 收听时长(同一口径),其余原因无跳点。
    /// 时钟异常拿不出结束时刻则本次不记。
    ///
    /// # Params:
    ///   - `finish_reason`: 结束原因(eof / skip / stop / error)
    ///   - `listen_ms`: 本次收听毫秒
    pub fn play_ended(&self, finish_reason: FinishReason, listen_ms: i64) {
        let Some(ended_at) = now_ms() else {
            return;
        };
        let skip_at_ms = matches!(finish_reason, FinishReason::Skip).then_some(listen_ms);
        self.send(StatsCommand::PlayEnded {
            ended_at,
            listen_ms,
            finish_reason,
            skip_at_ms,
        });
    }

    /// 记 daemon 启动(app_lifecycle;who=Daemon、actor=System)。经 [`Self::event`]
    /// 走 gating。
    ///
    /// # Params:
    ///   - `audio_backend`: 音频后端(device / null 降级)
    ///   - `session_restored`: 是否恢复了上次会话
    pub fn daemon_started(&self, audio_backend: AudioBackend, session_restored: bool) {
        self.event(StatsEvent::Behavior {
            actor: Actor::System,
            event: BehaviorEvent::AppLifecycle {
                who: LifecycleWho::Daemon,
                phase: LifecyclePhase::Start,
                audio_backend: Some(audio_backend),
                session_restored: Some(session_restored),
                client_version: None,
            },
        });
    }

    /// 记 daemon 停止(app_lifecycle;who=Daemon、actor=System)。启动富化字段(音频
    /// 后端 / 会话恢复)不适用于停止,落 NULL。
    pub fn daemon_stopped(&self) {
        self.event(StatsEvent::Behavior {
            actor: Actor::System,
            event: BehaviorEvent::AppLifecycle {
                who: LifecycleWho::Daemon,
                phase: LifecyclePhase::Stop,
                audio_backend: None,
                session_restored: None,
                client_version: None,
            },
        });
    }

    /// 排空栅栏:等 actor 处理完当前所有排队命令后返回。停机前调,保证末尾事件
    /// (如 daemon stop)真落库,不被进程退出截断。降级(无 actor)即刻返回。
    pub async fn flush(&self) {
        let Some(tx) = &self.tx else {
            return;
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        if tx.send(StatsCommand::Flush { ack: ack_tx }).await.is_err() {
            return; // actor 已退,无需等。
        }
        // actor drop ack 端(异常退出)也让 await 结束,不悬挂。
        let _ = ack_rx.await;
    }

    /// 记一次搜索(searches;行为域)。据 `search_queries` 档处理查询词:Off 整条不记、
    /// Hashed 丢原文只留散列、Raw 原文 + 散列都留;散列恒算(去重 / 保次数用)。
    ///
    /// # Params:
    ///   - `actor`: 发起方(user 界面搜索 / script 的 mineral.search)
    ///   - `raw_query`: 搜索词原文
    ///   - `kind`: 搜索目标类型
    ///   - `source`: 来源 name
    ///   - `page`: 翻页页码
    ///   - `result_count`: 结果条数(未知为 `None`)
    ///   - `outcome`: 结局
    #[allow(clippy::too_many_arguments)] // 搜索一行的固有列,拆结构体反增噪
    pub fn record_search(
        &self,
        actor: Actor,
        raw_query: &str,
        kind: SearchTargetKind,
        source: SourceKind,
        page: i64,
        result_count: Option<i64>,
        outcome: SearchOutcome,
    ) {
        let query = match self.params.load().search_queries() {
            SearchQueryMode::Off => return,
            SearchQueryMode::Hashed => None,
            SearchQueryMode::Raw => Some(raw_query.to_owned()),
        };
        self.event(StatsEvent::Behavior {
            actor,
            event: BehaviorEvent::Search {
                query,
                query_hash: query_hash(raw_query),
                kind,
                source,
                page,
                result_count,
                outcome,
            },
        });
    }

    /// 全谱事件:按 kind gating + 来源排除通过则送 actor。`exclude_sources` 在此对全谱
    /// 生效——被排除源的搜索 / 取数 / 取链等与 plays 一并无痕(配置承诺「完全不落库」)。
    /// 事件时刻在此 stamp(send 前打点,排队延迟不影响 ts);时钟异常本条不记。
    pub fn event(&self, event: StatsEvent) {
        {
            let params = self.params.load();
            if !params.collects_event(event.kind_name()) {
                return;
            }
            if event
                .source_name()
                .is_some_and(|source| params.excludes_source(source))
            {
                return;
            }
        }
        let Some(ts) = now_ms() else {
            return;
        };
        self.send(StatsCommand::Event {
            ts,
            event: Box::new(event),
        });
    }

    /// play_url 就绪后富化在播行的音频快照(整组覆盖);pending 缺席则 actor 侧丢弃。
    /// 起播已带的 playback_origin 不在此改。
    ///
    /// # Params:
    ///   - `audio`: 已生效播放 URL 的音频快照
    pub fn enrich_play_audio(&self, audio: PlayAudioSnapshot) {
        self.send(StatsCommand::EnrichAudio(audio));
    }

    /// 送命令;降级则静默丢弃,通道满 / 已关则丢弃 + 累计计数 + warn(埋点故障绝不回灌
    /// 播放路径:满则丢这条,不阻塞、不背压)。
    fn send(&self, cmd: StatsCommand) {
        let Some(tx) = &self.tx else {
            return;
        };
        if tx.try_send(cmd).is_err() {
            let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            mineral_log::warn!(target: "stats", dropped_total = total, "埋点通道满或已关,丢弃一条命令");
        }
    }
}

/// 常驻 writer actor:独占 store + 会话 tracker + 在播 pending,单消费者串行无锁。
/// 收到 [`StatsCommand::Shutdown`] 或丢尽 sender(`recv` 返回 `None`)后退出;退出前把
/// 在播 pending 按 `finish_reason=stop` 结算(在播行丢失由此收窄到仅硬 kill)。
async fn run(
    mut rx: mpsc::Receiver<StatsCommand>,
    store: StatsStore,
    params: Arc<ArcSwap<StatsParams>>,
) {
    let mut tracker = SessionTracker::default();
    // 在播语境 + 其归属会话 id。
    let mut pending: Option<(PendingPlay, i64)> = None;
    // retention 每日巡检:首拍延后一天(不在启动瞬间裁),之后每天一次。经同一 actor 走
    // prune,唯一 writer 不变量不破(§5.5)。错过的拍跳过而非补齐(prune 幂等)。
    let start = tokio::time::Instant::now() + Duration::from_secs(SECONDS_PER_DAY);
    let mut retention_tick = tokio::time::interval_at(start, Duration::from_secs(SECONDS_PER_DAY));
    retention_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            maybe_cmd = rx.recv() => {
                // 丢尽 sender → 退出(graceful shutdown 走这条,与显式 Shutdown 同义)。
                let Some(cmd) = maybe_cmd else { break };
                if !process_command(cmd, &store, &mut tracker, &mut pending, &params).await {
                    break;
                }
            }
            _ = retention_tick.tick() => run_retention(&store, &params).await,
        }
    }
    // 停机结算:在播 pending 按 stop 落一行,再退出。
    settle_pending_as_stop(&store, &mut pending).await;
}

/// 处理一条 actor 命令:会话推进 / pending 结算 / 事件落库 / 音频富化 / flush 应答。
///
/// # Return:
///   `true` 继续循环;`false`(收到 [`StatsCommand::Shutdown`])请求退出。
async fn process_command(
    cmd: StatsCommand,
    store: &StatsStore,
    tracker: &mut SessionTracker,
    pending: &mut Option<(PendingPlay, i64)>,
    params: &Arc<ArcSwap<StatsParams>>,
) -> bool {
    match cmd {
        StatsCommand::PlayStarted(started) => {
            // 防线:起播到达时仍有未结算 pending = 上游某条改播路径漏发 play_ended
            // (或结算命令被通道满丢弃)。不静默丢行——按 skip 自愈结算 + warn 暴露。
            // listen 用两次起播的 wall-clock 差近似(含暂停时段,偏大;有曲长则钳到曲长)。
            if let Some((snapshot, session_id)) = pending.take() {
                mineral_log::warn!(
                    target: "stats",
                    song_id = snapshot.song_id.as_str(),
                    "起播时发现未结算的在播行,按 skip 自愈结算(上游漏结算?)"
                );
                let ended_at = started.started_at;
                let mut listen_ms = ended_at.saturating_sub(snapshot.started_at).max(0);
                if let Some(duration) = snapshot.duration_ms_snapshot {
                    listen_ms = listen_ms.min(duration);
                }
                let record = assemble(
                    snapshot,
                    session_id,
                    ended_at,
                    listen_ms,
                    FinishReason::Skip,
                    Some(listen_ms),
                );
                if let Err(e) = store.record_play(&record).await {
                    mineral_log::warn!(target: "stats", error = chain(&e), "自愈结算 record_play 失败");
                }
                if let Err(e) = store.touch_session(session_id, ended_at).await {
                    mineral_log::warn!(target: "stats", error = chain(&e), "自愈结算 touch_session 失败");
                }
            }
            let gap_ms = params.load().gap_ms();
            *pending = assign_session(store, tracker, started.started_at, gap_ms)
                .await
                .map(|session_id| (*started, session_id));
        }
        StatsCommand::PlayEnded {
            ended_at,
            listen_ms,
            finish_reason,
            skip_at_ms,
        } => {
            if let Some((snapshot, session_id)) = pending.take() {
                let record = assemble(
                    snapshot,
                    session_id,
                    ended_at,
                    listen_ms,
                    finish_reason,
                    skip_at_ms,
                );
                if let Err(e) = store.record_play(&record).await {
                    mineral_log::warn!(target: "stats", error = chain(&e), "record_play 失败");
                }
                if let Err(e) = store.touch_session(session_id, ended_at).await {
                    mineral_log::warn!(target: "stats", error = chain(&e), "touch_session 失败");
                }
            }
        }
        StatsCommand::Event { ts, event } => {
            let session_id = tracker.current_id();
            if let Err(e) = store.record_event(ts, session_id, &event).await {
                mineral_log::warn!(target: "stats", error = chain(&e), "record_event 失败");
            }
        }
        StatsCommand::EnrichAudio(audio) => {
            // 富化在播行的音频快照(pending 缺席则丢弃);结算时随快照落库。
            if let Some((snapshot, _)) = pending.as_mut() {
                snapshot.audio = audio;
            }
        }
        // FIFO:走到这条时上面的命令都已 await 完成落库,应答即代表「已排空」。
        StatsCommand::Flush { ack } => {
            let _ = ack.send(());
        }
        // 请求退出:结算在播 pending 交由 run 循环退出后统一做。
        StatsCommand::Shutdown => return false,
    }
    true
}

/// 停机结算:在播 pending 按 `finish_reason=stop` 落一行(listen = now − started,钳非负),
/// 并 touch 其会话。无在播 pending 则 no-op;时钟异常拿不出结束时刻则丢弃在播行(warn,
/// 不用假时间戳造行)。
async fn settle_pending_as_stop(store: &StatsStore, pending: &mut Option<(PendingPlay, i64)>) {
    let Some((snapshot, session_id)) = pending.take() else {
        return;
    };
    let Some(ended_at) = now_ms() else {
        mineral_log::warn!(target: "stats", "系统时钟异常,停机结算丢弃在播行");
        return;
    };
    let listen_ms = ended_at.saturating_sub(snapshot.started_at).max(0);
    let record = assemble(
        snapshot,
        session_id,
        ended_at,
        listen_ms,
        FinishReason::Stop,
        None,
    );
    if let Err(e) = store.record_play(&record).await {
        mineral_log::warn!(target: "stats", error = chain(&e), "停机结算 record_play 失败");
    }
    if let Err(e) = store.touch_session(session_id, ended_at).await {
        mineral_log::warn!(target: "stats", error = chain(&e), "停机结算 touch_session 失败");
    }
}

/// retention 巡检:`Days(n)` 时裁掉 n 天前的流水;`Forever` 为 no-op。
async fn run_retention(store: &StatsStore, params: &Arc<ArcSwap<StatsParams>>) {
    let Retention::Days(days) = params.load().retention() else {
        return;
    };
    let Some(before_ms) = retention_before_ms(days) else {
        return;
    };
    if let Err(e) = store.prune(before_ms).await {
        mineral_log::warn!(target: "stats", error = chain(&e), "retention prune 失败");
    }
}

/// 保留 `days` 天的裁剪水位(now − days);系统时间异常 / 溢出返回 `None`(本轮不裁)。
fn retention_before_ms(days: u32) -> Option<i64> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let now_ms = i64::try_from(now_ms).ok()?;
    let window = i64::from(days).checked_mul(MS_PER_DAY)?;
    Some(now_ms.saturating_sub(window))
}

/// 会话推进:续旧则 touch ended_at,开新则 open + begin;disabled / 失败返回 `None`
/// (本次播放不记)。
async fn assign_session(
    store: &StatsStore,
    tracker: &mut SessionTracker,
    started_at: i64,
    gap_ms: i64,
) -> Option<i64> {
    match tracker.on_activity(started_at, gap_ms) {
        SessionDecision::Continue { session_id } => {
            if let Err(e) = store.touch_session(session_id, started_at).await {
                mineral_log::warn!(target: "stats", error = chain(&e), "续会话 touch 失败");
            }
            Some(session_id)
        }
        SessionDecision::StartNew => match store.open_session(started_at).await {
            Ok(Some(id)) => {
                tracker.begin(id, started_at);
                Some(id)
            }
            Ok(None) => None,
            Err(e) => {
                mineral_log::warn!(target: "stats", error = chain(&e), "开会话失败");
                None
            }
        },
    }
}

/// 起播快照 + 结束事实 → 完整 [`PlayRecord`]。
fn assemble(
    snapshot: PendingPlay,
    session_id: i64,
    ended_at: i64,
    listen_ms: i64,
    finish_reason: FinishReason,
    skip_at_ms: Option<i64>,
) -> PlayRecord {
    PlayRecord {
        song_id: snapshot.song_id,
        started_at: snapshot.started_at,
        ended_at,
        listen_ms,
        duration_ms_snapshot: snapshot.duration_ms_snapshot,
        finish_reason,
        skip_at_ms,
        play_mode: snapshot.play_mode,
        session_id,
        origin: snapshot.origin,
        actor: snapshot.actor,
        context: snapshot.context,
        audio: snapshot.audio,
        playback_origin: snapshot.playback_origin,
    }
}

#[cfg(test)]
mod tests {
    use super::{PendingPlay, StatsRecorder};
    use arc_swap::ArcSwap;
    use mineral_model::{SongId, SourceKind};
    use mineral_stats::{
        Actor, FinishReason, Level, PlayOrigin, PlaybackOrigin, QueueContext, Retention,
        SearchQueryMode, StatsParams, StatsStore,
    };
    use rustc_hash::{FxHashMap, FxHashSet};
    use std::sync::Arc;

    fn full_params() -> StatsParams {
        StatsParams::builder()
            .level(Level::Full)
            .collect(FxHashMap::default())
            .search_queries(SearchQueryMode::Raw)
            .exclude_sources(FxHashSet::default())
            .gap_ms(30 * 60_000)
            .retention(Retention::Forever)
            .build()
    }

    fn pending(started_at: i64) -> PendingPlay {
        PendingPlay {
            song_id: SongId::new(SourceKind::NETEASE, "1"),
            started_at,
            origin: PlayOrigin::Explicit,
            actor: Actor::User,
            context: QueueContext::Unknown,
            play_mode: mineral_stats::PlayMode::Sequential,
            duration_ms_snapshot: Some(200_000),
            audio: mineral_stats::PlayAudioSnapshot::default(),
            playback_origin: PlaybackOrigin::Remote,
        }
    }

    async fn temp_store() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    /// 与 full_params 同,但 retention 设为保留 `days` 天。
    fn days_params(days: u32) -> StatsParams {
        StatsParams::builder()
            .level(Level::Full)
            .collect(FxHashMap::default())
            .search_queries(SearchQueryMode::Raw)
            .exclude_sources(FxHashSet::default())
            .gap_ms(30 * 60_000)
            .retention(Retention::Days(days))
            .build()
    }

    /// 直落一行 `started_at` 处的播放事实(retention 测试造数)。
    async fn seed_play(store: &StatsStore, started_at: i64) -> color_eyre::Result<()> {
        let session_id = store
            .open_session(started_at)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("期望会话 id"))?;
        let record = super::assemble(
            pending(started_at),
            session_id,
            started_at,
            1000,
            FinishReason::Eof,
            None,
        );
        store.record_play(&record).await?;
        Ok(())
    }

    /// pending_from_start:origin 与 actor 两维独立穿透——用户按 next 是
    /// (AutoAdvance, User),脚本切歌是 (AutoAdvance, Script),EOF 自治是
    /// (AutoAdvance, System),不再从 origin 派生 actor。
    #[test]
    fn pending_from_start_carries_actor_and_origin() -> color_eyre::Result<()> {
        use mineral_stats::PlayOrigin;
        let mk = |origin, actor| {
            super::pending_from_start(
                SongId::new(SourceKind::NETEASE, "1"),
                mineral_stats::PlayMode::Sequential,
                Some(1000),
                mineral_protocol::PlaybackOrigin::Remote,
                origin,
                actor,
                QueueContext::Unknown,
            )
            .ok_or_else(|| color_eyre::eyre::eyre!("正常时钟下应产出快照"))
        };
        let explicit = mk(PlayOrigin::Explicit, Actor::User)?;
        assert_eq!(explicit.origin, PlayOrigin::Explicit);
        assert_eq!(explicit.actor, Actor::User);
        // 同一 origin 下 actor 可分道:用户按键 / 脚本 / 系统自治。
        assert_eq!(mk(PlayOrigin::AutoAdvance, Actor::User)?.actor, Actor::User);
        assert_eq!(
            mk(PlayOrigin::AutoAdvance, Actor::Script)?.actor,
            Actor::Script
        );
        assert_eq!(
            mk(PlayOrigin::AutoAdvance, Actor::System)?.actor,
            Actor::System
        );
        // context 原样穿透(队列语境继承)。
        let with_ctx = super::pending_from_start(
            SongId::new(SourceKind::NETEASE, "1"),
            mineral_stats::PlayMode::Sequential,
            None,
            mineral_protocol::PlaybackOrigin::Remote,
            PlayOrigin::Explicit,
            Actor::User,
            QueueContext::Playlist {
                id: mineral_model::PlaylistId::new(SourceKind::NETEASE, "7"),
            },
        )
        .ok_or_else(|| color_eyre::eyre::eyre!("正常时钟下应产出快照"))?;
        assert!(matches!(with_ctx.context, QueueContext::Playlist { .. }));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn records_play_start_to_end() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let (recorder, handle) = StatsRecorder::spawn(store.clone(), full_params());
        recorder.play_started(pending(1000));
        recorder.play_ended(FinishReason::Eof, /*listen_ms*/ 3000);
        drop(recorder); // 关通道 → actor 排空退出
        handle.await?;
        let totals = store.totals(0..i64::MAX).await?;
        assert_eq!(totals.plays, 1);
        assert_eq!(totals.listen_ms, 3000);
        assert_eq!(totals.completed, 1, "eof");
        Ok(())
    }

    /// 自愈防线:连续两次 play_started 而中间无 play_ended(上游漏结算的直接改播),第一
    /// 首不得静默丢——按 skip 自愈落一行,第二首照常配对结算,总计两行。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsettled_pending_self_heals_as_skip() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let (recorder, handle) = StatsRecorder::spawn(store.clone(), full_params());
        recorder.play_started(pending(1000));
        recorder.play_started(pending(60_000)); // 漏结算的改播
        recorder.play_ended(FinishReason::Eof, /*listen_ms*/ 50_000);
        drop(recorder);
        handle.await?;
        let totals = store.totals(0..i64::MAX).await?;
        assert_eq!(totals.plays, 2, "被顶掉的第一首自愈落行,不丢");
        assert_eq!(totals.completed, 1, "自愈行按 skip 计,非完播");
        Ok(())
    }

    /// exclude_sources 全谱语义:被排除源不止 plays,携带该源的行为 / 系统事件一并无痕
    /// (配置承诺「完全不落库」);无来源归属的全局事件不受影响。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exclude_sources_gates_events_too() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let params = StatsParams::builder()
            .level(Level::Full)
            .collect(FxHashMap::default())
            .search_queries(SearchQueryMode::Raw)
            .exclude_sources(std::iter::once("netease".to_owned()).collect::<FxHashSet<String>>())
            .gap_ms(30 * 60_000)
            .retention(Retention::Forever)
            .build();
        let (recorder, handle) = StatsRecorder::spawn(store.clone(), params);
        // 被排除源的搜索(source 字段)与 seek(song namespace)都不落。
        recorder.event(mineral_stats::StatsEvent::Behavior {
            actor: Actor::User,
            event: mineral_stats::BehaviorEvent::Search {
                query: Some("q".to_owned()),
                query_hash: "h".to_owned(),
                kind: mineral_stats::SearchTargetKind::Song,
                source: SourceKind::NETEASE,
                page: 0,
                result_count: Some(5),
                outcome: mineral_stats::SearchOutcome::Ok,
            },
        });
        recorder.event(mineral_stats::StatsEvent::Behavior {
            actor: Actor::User,
            event: mineral_stats::BehaviorEvent::Seek {
                song: SongId::new(SourceKind::NETEASE, "1"),
                from_ms: 0,
                to_ms: 5000,
            },
        });
        // 无来源归属的全局事件(音量)不受排除影响。
        recorder.event(mineral_stats::StatsEvent::Behavior {
            actor: Actor::User,
            event: mineral_stats::BehaviorEvent::VolumeChange {
                from_pct: 50,
                to_pct: 60,
            },
        });
        drop(recorder);
        handle.await?;
        assert_eq!(store.status().await?.events, 1, "仅无来源的音量事件落库");
        Ok(())
    }

    /// 起播语境的搜索词同受 search_queries 档约束:Off 档下 plays.context_ref 落 NULL
    /// (kind 仍 search),搜索词不经语境旁路落库。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn play_context_search_query_redacted_by_mode() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let params = StatsParams::builder()
            .level(Level::Full)
            .collect(FxHashMap::default())
            .search_queries(SearchQueryMode::Off)
            .exclude_sources(FxHashSet::default())
            .gap_ms(30 * 60_000)
            .retention(Retention::Forever)
            .build();
        let (recorder, handle) = StatsRecorder::spawn(store.clone(), params);
        let mut p = pending(1000);
        p.context = QueueContext::Search {
            query: Some("敏感词".to_owned()),
        };
        recorder.play_started(p);
        recorder.play_ended(FinishReason::Eof, /*listen_ms*/ 3000);
        drop(recorder);
        handle.await?;
        let contexts = store.top_contexts(0..i64::MAX, None, 10).await?;
        let first = contexts
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("期望一条语境"))?;
        assert_eq!(first.kind, "search", "kind 保留");
        assert_eq!(first.reference, None, "Off 档搜索词不落 context_ref");
        Ok(())
    }

    /// enrich_play_audio 在起播后富化在播行的音频快照,结算时随记落库(经 distributions
    /// 的 by_format 验证 flac 落进了 audio_format 列)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enrich_play_audio_fills_format() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let (recorder, handle) = StatsRecorder::spawn(store.clone(), full_params());
        recorder.play_started(pending(1000));
        recorder.enrich_play_audio(mineral_stats::PlayAudioSnapshot {
            audio_format: Some(mineral_model::AudioFormat::Flac),
            bitrate_bps: Some(900_000),
            quality: Some(mineral_model::BitRate::Lossless),
            bit_depth: Some(16),
            substituted: false,
        });
        recorder.play_ended(FinishReason::Eof, /*listen_ms*/ 3000);
        drop(recorder);
        handle.await?;
        let dist = store.distributions(0..i64::MAX).await?;
        assert!(
            dist.by_format
                .iter()
                .any(|s| s.value == "flac" && s.plays == 1),
            "by_format 应含 flac:{:?}",
            dist.by_format
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn records_event_through_actor() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let (recorder, handle) = StatsRecorder::spawn(store.clone(), full_params());
        recorder.event(mineral_stats::StatsEvent::Behavior {
            actor: mineral_stats::Actor::User,
            event: mineral_stats::BehaviorEvent::Search {
                query: Some("q".to_owned()),
                query_hash: "h".to_owned(),
                kind: mineral_stats::SearchTargetKind::Song,
                source: SourceKind::NETEASE,
                page: 0,
                result_count: Some(5),
                outcome: mineral_stats::SearchOutcome::Ok,
            },
        });
        drop(recorder);
        handle.await?;
        assert_eq!(store.status().await?.events, 1, "事件经 actor 落库");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn off_level_records_nothing() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let (recorder, handle) = StatsRecorder::spawn(store.clone(), super::off_params());
        recorder.play_started(pending(1000));
        recorder.play_ended(FinishReason::Eof, /*listen_ms*/ 3000);
        drop(recorder);
        handle.await?;
        assert_eq!(store.totals(0..i64::MAX).await?.plays, 0, "off 档零写入");
        Ok(())
    }

    /// retention Days(n):裁掉 n 天前的流水,保留窗口内的(§5.5;直调巡检,不等 timer)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retention_days_prunes_ancient_keeps_recent() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        seed_play(&store, 1000).await?; // 远古(1970),必被裁
        let now_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        seed_play(&store, now_ms).await?; // 此刻,必留
        let params = Arc::new(ArcSwap::from_pointee(days_params(1)));
        super::run_retention(&store, &params).await;
        let plays = store.recent_plays(0..i64::MAX, None, 10).await?;
        assert_eq!(plays.len(), 1, "只应留下窗口内的一行");
        let kept = plays
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("期望留下一行"))?;
        assert_eq!(kept.started_at, now_ms, "留下的是此刻那行");
        Ok(())
    }

    /// retention Forever:一行不裁(默认永久)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retention_forever_prunes_nothing() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        seed_play(&store, 1000).await?; // 远古也不裁
        let params = Arc::new(ArcSwap::from_pointee(full_params()));
        super::run_retention(&store, &params).await;
        assert_eq!(
            store.totals(0..i64::MAX).await?.plays,
            1,
            "Forever 一行不裁"
        );
        Ok(())
    }

    /// daemon 生命周期 start/stop 经 actor 落 app_lifecycle;`flush` 是真排空栅栏:
    /// **不 drop / 不 join** 也能保证两条已落库(status 计入 app_lifecycle)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn daemon_lifecycle_flush_is_a_barrier() -> color_eyre::Result<()> {
        use mineral_stats::AudioBackend;
        let (_dir, store) = temp_store().await?;
        let (recorder, _handle) = StatsRecorder::spawn(store.clone(), full_params());
        recorder.daemon_started(AudioBackend::Null, /*session_restored*/ true);
        recorder.daemon_stopped();
        // 关键:此处不 drop recorder、不 await handle,仅靠 flush 保证已排空落库。
        recorder.flush().await;
        assert_eq!(
            store.status().await?.events,
            2,
            "start + stop 两条应在 flush 返回前落库"
        );
        Ok(())
    }

    /// record_search 的 search_queries 档门控:Off 整条不记、Raw 记一条。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn search_query_mode_gates_recording() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let off = StatsParams::builder()
            .level(Level::Full)
            .collect(FxHashMap::default())
            .search_queries(SearchQueryMode::Off)
            .exclude_sources(FxHashSet::default())
            .gap_ms(30 * 60_000)
            .retention(Retention::Forever)
            .build();
        let (rec_off, h_off) = StatsRecorder::spawn(store.clone(), off);
        rec_off.record_search(
            Actor::User,
            "q",
            mineral_stats::SearchTargetKind::Song,
            SourceKind::NETEASE,
            0,
            Some(3),
            mineral_stats::SearchOutcome::Ok,
        );
        drop(rec_off);
        h_off.await?;
        assert_eq!(store.status().await?.events, 0, "Off 档搜索整条不记");

        let (rec_raw, h_raw) = StatsRecorder::spawn(store.clone(), full_params());
        rec_raw.record_search(
            Actor::User,
            "q",
            mineral_stats::SearchTargetKind::Song,
            SourceKind::NETEASE,
            0,
            Some(3),
            mineral_stats::SearchOutcome::Ok,
        );
        drop(rec_raw);
        h_raw.await?;
        assert_eq!(store.status().await?.events, 1, "Raw 档记一条搜索");
        Ok(())
    }

    /// 通道打满:超出 bounded 容量的命令被丢弃并计数。current_thread 下 actor 尚未被 poll,
    /// 同步猛灌必然填满缓冲 → 丢弃计数确定。故障域隔离:丢的是埋点,绝不背压播放路径。
    #[tokio::test]
    async fn channel_full_increments_drop_counter() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let (recorder, _actor) = StatsRecorder::spawn(store, full_params());
        let cap = i64::try_from(super::CHANNEL_CAPACITY)?;
        let overflow = 100_i64;
        // play_ended 不 gate、同步 try_send:前 cap 条入缓冲,后 overflow 条丢弃。
        for _ in 0..(cap + overflow) {
            recorder.play_ended(FinishReason::Stop, /*listen_ms*/ 0);
        }
        assert_eq!(
            recorder.dropped_count(),
            u64::try_from(overflow)?,
            "超出容量的都计入丢弃计数"
        );
        Ok(())
    }

    /// 优雅停机:actor 收到 Shutdown 后把在播 pending 按 `finish_reason=stop` 结算落库
    /// (在播行丢失由此收窄到仅硬 kill)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_settles_in_play_pending_as_stop() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let (recorder, actor) = StatsRecorder::spawn(store.clone(), full_params());
        recorder.play_started(pending(1000)); // 起播,不发 play_ended
        recorder.flush().await; // 栅栏:保证 PlayStarted 已处理、pending 已建
        recorder.shutdown().await; // 发 Shutdown
        actor.await?; // 等 actor 结算在播 pending + 退出
        let plays = store.recent_plays(0..i64::MAX, None, 10).await?;
        assert_eq!(plays.len(), 1, "在播 pending 结算为一行");
        let row = plays
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("期望结算行"))?;
        assert_eq!(row.finish_reason, FinishReason::Stop, "结算为 stop");
        assert_eq!(row.started_at, 1000, "结算的是在播那首(起播时刻不变)");
        Ok(())
    }

    /// 无在播 pending 时停机:不凭空造行(结算 no-op)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_without_pending_writes_nothing() -> color_eyre::Result<()> {
        let (_dir, store) = temp_store().await?;
        let (recorder, actor) = StatsRecorder::spawn(store.clone(), full_params());
        recorder.shutdown().await;
        actor.await?;
        assert_eq!(store.totals(0..i64::MAX).await?.plays, 0, "无在播行不造行");
        Ok(())
    }
}

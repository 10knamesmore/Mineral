//! client 侧只读状态镜像:daemon 是业务真相,镜像只接收版本化订阅更新。
//!
//! 状态应用与读取约定:
//! - **版本门控**:旧版本 / 重复版本丢弃,不倒退;增量缺基准时报 `Resync`。
//! - **未就绪与已知为空可区分**:`Option` 表达「尚未收到」,空集合表达「已知为空」。
//! - **播放位置本地推进**:锚点 + 本地单调钟,不直接相减两进程的 `Instant`。
//! - **PCM 近期窗口有界**:落后跳窗并留下缺口事实,不无限追旧样本。

use std::collections::VecDeque;
use std::time::Instant;

use mineral_audio::AudioSnapshot;
use mineral_protocol::{
    DownloadDetailUpdate, DownloadId, DownloadStatus, DownloadSummary, Event, PlayCursor, PlayMode,
    PlaybackOrigin, PlayerSync, PlayerVersions, SongDownloadView, SubscriptionId,
    SubscriptionTopic, UpdatePayload,
};
use mineral_task::Snapshot;
use parking_lot::RwLock;
use rustc_hash::FxHashMap;

use super::pcm::PcmWindow;

/// 订阅更新在镜像上的应用结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApplyOutcome {
    /// 已应用。
    Applied,

    /// 版本陈旧 / 重复,已忽略。
    Ignored,

    /// 增量缺基准(版本跳跃),需要重新快照。
    Resync,
}

/// 窗口标题覆盖的镜像状态:区分「尚未收到」与「已知无覆盖」。
#[derive(Clone, Debug, Default)]
pub enum WindowTitleOverride {
    /// 尚未收到任何覆盖状态。
    #[default]
    NotKnown,

    /// 已知状态:`Some` 为覆盖文本,`None` 为无覆盖(回落模板)。
    Set(Option<String>),
}

/// 播放镜像:队列 / 当前曲 / 轻段。
#[derive(Clone, Debug, Default)]
pub struct PlayerMirror {
    /// 已应用的版本号(回报 / 差量判断)。
    versions: PlayerVersions,

    /// 当前曲在队列中的锚点。
    cursor: PlayCursor,

    /// 播放模式。
    play_mode: PlayMode,

    /// 在播音频来源。
    play_origin: Option<PlaybackOrigin>,

    /// 队列(顺序模式 = 原序;shuffle = 洗过)。
    queue: Vec<mineral_model::Song>,

    /// shuffle 原序。
    original_queue: Option<Vec<mineral_model::Song>>,

    /// 当前曲重段(收到过 current 段才有值)。
    current: Option<mineral_protocol::CurrentSync>,
}

impl PlayerMirror {
    /// 已应用版本。
    #[must_use]
    pub fn versions(&self) -> PlayerVersions {
        self.versions
    }

    /// 当前曲锚点。
    #[must_use]
    pub fn cursor(&self) -> PlayCursor {
        self.cursor
    }

    /// 播放模式。
    #[must_use]
    pub fn play_mode(&self) -> PlayMode {
        self.play_mode
    }

    /// 在播来源。
    #[must_use]
    pub fn play_origin(&self) -> Option<PlaybackOrigin> {
        self.play_origin
    }

    /// 队列。
    #[must_use]
    pub fn queue(&self) -> &[mineral_model::Song] {
        &self.queue
    }

    /// shuffle 原序。
    #[must_use]
    pub fn original_queue(&self) -> Option<&[mineral_model::Song]> {
        self.original_queue.as_deref()
    }

    /// 当前曲重段;`None` = 尚未收到。
    #[must_use]
    pub fn current(&self) -> Option<&mineral_protocol::CurrentSync> {
        self.current.as_ref()
    }

    /// 应用一段播放同步(重段缺席 = 与已有一致,不清空)。
    ///
    /// # Params:
    ///   - `sync`: 版本门控结果
    fn apply(&mut self, sync: PlayerSync) {
        self.versions = sync.versions;
        self.cursor = sync.cursor;
        self.play_mode = sync.play_mode;
        self.play_origin = sync.play_origin;
        if let Some(queue) = sync.queue {
            self.queue = queue.queue;
            self.original_queue = queue.original_queue;
        }
        if let Some(current) = sync.current {
            self.current = Some(current);
        }
    }

    /// 队列重段是否已到过(供 UI 区分「尚未就绪」与「已知为空」)。
    #[must_use]
    pub fn queue_ready(&self) -> bool {
        !self.versions.queue.is_initial()
    }
}

/// 播放锚点镜像:位置按本地单调钟推进。
#[derive(Clone, Debug)]
pub struct PlaybackMirror {
    /// 最近一次收到的权威锚点。
    anchor: AudioSnapshot,

    /// 锚点到达的本地时刻。
    received_at: Instant,
}

impl Default for PlaybackMirror {
    fn default() -> Self {
        Self {
            anchor: AudioSnapshot::default(),
            received_at: Instant::now(),
        }
    }
}

impl PlaybackMirror {
    /// 最近一次权威锚点。
    #[must_use]
    pub fn anchor(&self) -> &AudioSnapshot {
        &self.anchor
    }

    /// 展示用播放位置:播放中用本地单调钟推进,暂停 / 停止时冻结在锚点值。
    ///
    /// # Params:
    ///   - `now`: 当前本地单调时刻
    #[must_use]
    pub fn position_ms(&self, now: Instant) -> u64 {
        if !self.anchor.playing {
            return self.anchor.position_ms;
        }
        let elapsed_ms = now.saturating_duration_since(self.received_at).as_millis();
        let elapsed_ms = u64::try_from(elapsed_ms).unwrap_or(u64::MAX);
        let advanced = self.anchor.position_ms.saturating_add(elapsed_ms);
        // 时长已知时钳在时长内,避免校准间隙越过曲终。
        self.anchor
            .duration_ms
            .map_or(advanced, |duration| advanced.min(duration))
    }

    /// 应用新锚点。
    ///
    /// # Params:
    ///   - `snapshot`: daemon 的权威快照
    ///   - `now`: 到达时刻
    fn apply(&mut self, snapshot: AudioSnapshot, now: Instant) {
        self.anchor = snapshot;
        self.received_at = now;
    }
}

/// 下载明细镜像:顺序 + 行表,静态字段与进度分离。
#[derive(Clone, Debug, Default)]
pub struct DownloadsDetailMirror {
    /// 展示顺序(active / queued / 最近终态)。
    order: Vec<DownloadId>,

    /// 行表。
    rows: FxHashMap<DownloadId, SongDownloadView>,
}

impl DownloadsDetailMirror {
    /// 按展示顺序返回行。
    #[must_use]
    pub fn rows(&self) -> Vec<SongDownloadView> {
        self.order
            .iter()
            .filter_map(|id| self.rows.get(id))
            .cloned()
            .collect()
    }

    /// 行数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// 整表替换(快照)。
    ///
    /// # Params:
    ///   - `rows`: 权威快照(已按展示序排好)
    fn replace(&mut self, rows: Vec<SongDownloadView>) {
        self.order = rows.iter().map(|row| row.id.clone()).collect();
        self.rows = rows.into_iter().map(|row| (row.id.clone(), row)).collect();
    }

    /// 替换展示顺序(增量包携带的顺序变化)。
    ///
    /// # Params:
    ///   - `order`: 权威顺序
    fn set_order(&mut self, order: Vec<DownloadId>) {
        self.order = order;
    }

    /// 新增 / 静态字段变化(整行替换;不在顺序表里则追加到尾部)。
    ///
    /// # Params:
    ///   - `row`: 权威行
    fn upsert(&mut self, row: SongDownloadView) {
        if !self.order.iter().any(|id| id == &row.id) {
            self.order.push(row.id.clone());
        }
        self.rows.insert(row.id.clone(), row);
    }

    /// 轻量进度变化(只改动态字段)。
    ///
    /// # Params:
    ///   - `update`: 增量
    fn progress(
        &mut self,
        id: &DownloadId,
        status: DownloadStatus,
        bytes_done: u64,
        bytes_total: Option<u64>,
        speed_bps: u64,
        failure: Option<String>,
    ) {
        if let Some(row) = self.rows.get_mut(id) {
            row.status = status;
            row.bytes_done = bytes_done;
            row.bytes_total = bytes_total;
            row.speed_bps = speed_bps;
            row.failure = failure;
        }
    }

    /// 移除一行(历史裁剪)。
    ///
    /// # Params:
    ///   - `id`: 目标行
    fn remove(&mut self, id: &DownloadId) {
        self.order.retain(|existing| existing != id);
        self.rows.remove(id);
    }
}

/// 一条已登记订阅的主题与接收进度,退订时整体移除。
struct SubscriptionState {
    /// 订阅主题,决定载荷是否要求连续版本。
    topic: SubscriptionTopic,

    /// 最近应用的正版本号;`None` 表示尚未收到更新或正在重同步。
    last_version: Option<u64>,
}

/// 镜像内部可变状态。
struct MirrorState {
    /// 播放镜像。
    player: PlayerMirror,

    /// 播放锚点镜像。
    playback: PlaybackMirror,

    /// 任务摘要(尚未收到为 `None`)。
    tasks: Option<Snapshot>,

    /// 下载摘要。
    downloads_summary: DownloadSummary,

    /// 下载明细(订阅后才有值)。
    downloads_detail: Option<DownloadsDetailMirror>,

    /// 窗口标题覆盖状态。
    window_title: WindowTitleOverride,

    /// 待消费事件。
    events: VecDeque<Event>,

    /// 因容量丢弃的事件数。
    events_dropped: u64,

    /// PCM 近期窗口。
    pcm: PcmWindow,

    /// 链路是否可用。
    connected: bool,

    /// 已登记订阅及其接收进度;未登记 id 的更新被忽略。
    subscriptions: FxHashMap<SubscriptionId, SubscriptionState>,
}

/// 会话维护的本地状态镜像：调用方读取状态，集中消费事件与 PCM。
///
/// 实例由会话在连接时创建；订阅更新负责推进状态，调用方不能注入或覆盖业务事实。
pub struct Mirror {
    /// 受锁保护的镜像状态。
    state: RwLock<MirrorState>,

    /// 事件缓冲上限。
    event_capacity: usize,
}

impl Mirror {
    /// 构造空镜像。
    ///
    /// # Params:
    ///   - `event_capacity`: 待消费事件上限(超出丢最旧并计数)
    ///   - `pcm_window`: PCM 近期窗口样本上限
    #[must_use]
    pub(crate) fn new(event_capacity: usize, pcm_window: usize) -> Self {
        Self {
            state: RwLock::new(MirrorState {
                player: PlayerMirror::default(),
                playback: PlaybackMirror::default(),
                tasks: None,
                downloads_summary: DownloadSummary::default(),
                downloads_detail: None,
                window_title: WindowTitleOverride::NotKnown,
                events: VecDeque::new(),
                events_dropped: 0,
                pcm: PcmWindow::new(pcm_window),
                connected: true,
                subscriptions: FxHashMap::default(),
            }),
            event_capacity: event_capacity.max(1),
        }
    }

    /// 只读访问播放镜像。
    ///
    /// # Params:
    ///   - `f`: 读取闭包
    pub fn read_player<R>(&self, f: impl FnOnce(&PlayerMirror) -> R) -> R {
        f(&self.state.read().player)
    }

    /// 只读访问播放锚点镜像。
    ///
    /// # Params:
    ///   - `f`: 读取闭包
    pub fn read_playback<R>(&self, f: impl FnOnce(&PlaybackMirror) -> R) -> R {
        f(&self.state.read().playback)
    }

    /// 任务摘要快照(尚未收到为 `None`)。
    #[must_use]
    pub fn tasks_snapshot(&self) -> Option<Snapshot> {
        self.state.read().tasks.clone()
    }

    /// 只读访问下载摘要。
    ///
    /// # Params:
    ///   - `f`: 读取闭包
    pub fn read_downloads_summary<R>(&self, f: impl FnOnce(&DownloadSummary) -> R) -> R {
        f(&self.state.read().downloads_summary)
    }

    /// 下载明细快照(未订阅 / 尚未收到为 `None`)。
    #[must_use]
    pub fn downloads_detail_snapshot(&self) -> Option<DownloadsDetailMirror> {
        self.state.read().downloads_detail.clone()
    }

    /// 只读访问下载明细;未订阅 / 尚未收到快照时为 `None`。
    ///
    /// # Params:
    ///   - `f`: 读取闭包
    pub fn read_downloads_detail<R>(
        &self,
        f: impl FnOnce(Option<&DownloadsDetailMirror>) -> R,
    ) -> R {
        f(self.state.read().downloads_detail.as_ref())
    }

    /// 窗口标题覆盖状态。
    #[must_use]
    pub fn window_title_override(&self) -> WindowTitleOverride {
        self.state.read().window_title.clone()
    }

    /// 设置链路状态。
    ///
    /// # Params:
    ///   - `connected`: 是否可用
    pub(crate) fn set_connected(&self, connected: bool) {
        self.state.write().connected = connected;
    }

    /// 因容量丢弃的事件数。
    #[must_use]
    pub fn events_dropped(&self) -> u64 {
        self.state.read().events_dropped
    }
    /// 取走全部待消费事件(消费式)。
    #[must_use]
    pub fn drain_events(&self) -> Vec<Event> {
        let mut state = self.state.write();
        state.events.drain(..).collect()
    }

    /// 取走 PCM 近期窗口；断续标记须另行调用 [`Self::take_pcm_discontinuity`] 消费。
    #[must_use]
    pub fn drain_pcm(&self) -> Vec<f32> {
        self.state.write().pcm.drain()
    }
    /// 是否有待处理的 PCM 断续 / 换代(取后清除)。
    #[must_use]
    pub fn take_pcm_discontinuity(&self) -> bool {
        self.state.write().pcm.take_discontinuity()
    }

    /// 请求重同步后清空该订阅的版本基线:下一次完整快照重新建立基准。
    ///
    /// 与 [`Self::unregister_subscription`] 的区别是保留主题登记,后续增量仍能识别语义。
    ///
    /// # Params:
    ///   - `id`: 订阅 id
    pub(crate) fn reset_subscription(&self, id: SubscriptionId) {
        if let Some(subscription) = self.state.write().subscriptions.get_mut(&id) {
            subscription.last_version = None;
        }
    }

    /// 订阅是否已收到首帧(用于 CLI 等待自举 / 诊断就绪)。
    ///
    /// # Params:
    ///   - `id`: 订阅 id
    #[must_use]
    pub fn subscription_seen(&self, id: SubscriptionId) -> bool {
        self.state
            .read()
            .subscriptions
            .get(&id)
            .is_some_and(|subscription| subscription.last_version.is_some())
    }

    /// 登记订阅主题(收到首个更新前即可识别增量语义)。
    ///
    /// # Params:
    ///   - `id`: 订阅 id
    ///   - `topic`: 主题
    pub(crate) fn register_subscription(&self, id: SubscriptionId, topic: SubscriptionTopic) {
        self.state
            .write()
            .subscriptions
            .entry(id)
            .and_modify(|subscription| subscription.topic = topic)
            .or_insert(SubscriptionState {
                topic,
                last_version: None,
            });
    }

    /// 注销订阅(丢弃其版本记录,旧 id 的迟到更新从此被忽略)。
    ///
    /// # Params:
    ///   - `id`: 订阅 id
    pub(crate) fn unregister_subscription(&self, id: SubscriptionId) {
        let mut state = self.state.write();
        if state
            .subscriptions
            .remove(&id)
            .is_some_and(|subscription| subscription.topic == SubscriptionTopic::DownloadsDetail)
        {
            state.downloads_detail = None;
        }
    }

    /// 应用一条订阅更新。
    ///
    /// # Params:
    ///   - `id`: 订阅 id
    ///   - `version`: 该订阅的逻辑版本
    ///   - `payload`: 更新载荷
    ///
    /// # Return:
    ///   应用结论(版本陈旧 / 缺基准 / 已应用)。
    pub(crate) fn apply_update(
        &self,
        id: SubscriptionId,
        version: u64,
        payload: UpdatePayload,
    ) -> ApplyOutcome {
        let mut guard = self.state.write();
        let state = &mut *guard;
        let Some(subscription) = state.subscriptions.get_mut(&id) else {
            // 未知 / 已取消的订阅:迟到更新直接忽略,不污染镜像。
            return ApplyOutcome::Ignored;
        };
        if version == 0
            || subscription
                .last_version
                .is_some_and(|last| version <= last)
        {
            return ApplyOutcome::Ignored;
        }
        let incremental = matches!(
            (subscription.topic, &payload),
            (SubscriptionTopic::Player, UpdatePayload::Player { .. })
                | (
                    SubscriptionTopic::DownloadsDetail,
                    UpdatePayload::DownloadsDetailDelta(_)
                )
        );
        let next_version = subscription
            .last_version
            .map_or(/*default*/ 1, |last| last.saturating_add(1));
        if incremental && version != next_version {
            return ApplyOutcome::Resync;
        }
        let now = Instant::now();
        match payload {
            UpdatePayload::Player { sync, .. } => state.player.apply(*sync),
            UpdatePayload::PlayerQueuePart { .. } => {
                // 分片未组装完整不应单独到达;忽略并请求重同步。
                return ApplyOutcome::Resync;
            }
            UpdatePayload::Playback(snapshot) => state.playback.apply(*snapshot, now),
            UpdatePayload::Tasks(snapshot) => state.tasks = Some(*snapshot),
            UpdatePayload::DownloadsSummary(summary) => state.downloads_summary = summary,
            UpdatePayload::DownloadsDetailSnapshot(rows) => {
                state
                    .downloads_detail
                    .get_or_insert_with(DownloadsDetailMirror::default)
                    .replace(rows);
            }
            UpdatePayload::DownloadsDetailHead { .. } => return ApplyOutcome::Resync,
            UpdatePayload::DownloadsDetailPart { .. } => return ApplyOutcome::Resync,
            UpdatePayload::DownloadsDetailDelta(delta) => {
                let detail = state
                    .downloads_detail
                    .get_or_insert_with(DownloadsDetailMirror::default);
                if let Some(order) = delta.order {
                    detail.set_order(order);
                }
                for change in delta.changes {
                    match change {
                        DownloadDetailUpdate::Upsert(row) => detail.upsert(*row),
                        DownloadDetailUpdate::Progress {
                            id,
                            status,
                            bytes_done,
                            bytes_total,
                            speed_bps,
                            failure,
                        } => {
                            detail.progress(
                                &id,
                                status,
                                bytes_done,
                                bytes_total,
                                speed_bps,
                                failure,
                            );
                        }
                        DownloadDetailUpdate::Remove(id) => detail.remove(&id),
                    }
                }
            }
            UpdatePayload::Pcm(chunk) => state.pcm.push(&chunk),
            UpdatePayload::Event(event) => {
                // 事件本身只排队;只有窗口标题覆盖需要镜像一份当前值。
                match &*event {
                    Event::WindowTitleOverride { text } => {
                        state.window_title = WindowTitleOverride::Set(text.clone());
                    }
                    Event::Toast { .. }
                    | Event::Card { .. }
                    | Event::PropertyChanged { .. }
                    | Event::TrackFinished { .. }
                    | Event::DownloadCompleted { .. }
                    | Event::StoreChanged { .. }
                    | Event::ScriptReloaded
                    | Event::BusMessage { .. }
                    | Event::ConfigChanged { .. }
                    | Event::DismissToast { .. }
                    | Event::Task(_) => {}
                }
                state.events.push_back(*event);
                while state.events.len() > self.event_capacity {
                    state.events.pop_front();
                    state.events_dropped = state.events_dropped.saturating_add(1);
                }
            }
        }
        subscription.last_version = Some(version);
        ApplyOutcome::Applied
    }

    /// 将本地查询完成通知放入待消费事件队列。
    ///
    /// # Params:
    ///   - `event`: 事件
    pub(crate) fn push_event(&self, event: Event) {
        let mut state = self.state.write();
        state.events.push_back(event);
        while state.events.len() > self.event_capacity {
            state.events.pop_front();
            state.events_dropped = state.events_dropped.saturating_add(1);
        }
    }
}

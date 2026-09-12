//! 共享播放上下文与服务句柄，以及状态同步和只读配置查询。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mineral_audio::AudioHandle;
use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, SourceKind};
use mineral_persist::ServerStore;
use mineral_playback::PlaybackRegistry;
use mineral_protocol::{PlayerSync, PlayerVersions};
use mineral_task::{Priority, Scheduler, Snapshot, TaskKind};
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::download;
use crate::media_cache::MediaCache;
use crate::state::State;

/// 服务端 PlayerCore。`Clone` 通过 `Arc` 廉价。
#[derive(Clone)]
pub struct PlayerCore {
    /// 共享内部状态(audio handle / scheduler / 注入 channel / 播放上下文)。
    pub(crate) inner: Arc<Inner>,
}

/// `PlayerCore` 的真实状态。
pub(crate) struct Inner {
    /// 底层音频引擎句柄。
    pub(crate) audio: AudioHandle,

    /// 任务调度器(用于提交 Lyrics / Playlists 等 channel 取数任务)。
    pub(crate) scheduler: Scheduler,

    /// 已注入的 channel 列表(用于按 [`SourceKind`] 路由)。
    pub(super) channels: Vec<Arc<dyn MusicChannel>>,

    /// Playback providers keyed by source identity.
    pub(super) playback: PlaybackRegistry,

    /// 持久化句柄(廉价 clone,Arc 内部)。
    pub(crate) persist: ServerStore,

    /// 音频本体缓存(命中直接本地播、播完入缓存);禁用环境为 null-object。
    pub(super) media_cache: Arc<MediaCache>,

    /// 永久下载导出根目录；播放与幂等检查按歌曲元数据派生路径。
    pub(super) music_dir: Option<std::path::PathBuf>,

    /// Session-only Song download lifecycle owner。
    pub(crate) downloads: download::DownloadManager,

    /// 打标队列投递端:下载 / 缓存落盘后投一曲(开关关闭为 null-object,见 [`crate::tagging`])。
    pub(super) tagging: crate::tagging::TaggingQueue,

    /// 事件通知双路出口(event hub + 脚本线程)。
    pub(crate) notify: crate::notify::Notifier,

    /// 埋点 recorder(热路径 gating + fire-and-forget 落库);无持久化时为 disabled no-op。
    pub(crate) stats: crate::StatsRecorder,

    /// 属性 diff 的上次值缓存(background_loop 每 tick 比对)。
    pub(crate) props: crate::props::PropsWatch,

    /// 各连接上报的终端 UI 状态(`Request::TerminalState` 写、断开清;
    /// check_props 每 tick 采样灌 `terminal` 属性,last-wins 裁决见
    /// [`TerminalStates`](crate::props::TerminalStates))。
    pub(crate) ui_state: Mutex<crate::props::TerminalStates>,

    /// 有效配置宿主(合成底树 + session 覆盖 + 窗口标题覆盖,见 [`crate::config_host`])。
    pub(crate) config_host: crate::config_host::ConfigHost,

    /// 播放上下文(队列/当前歌/歌词/预拉状态)。
    pub(crate) state: Mutex<State>,

    /// 播放状态变更订阅端(订阅者被唤醒后按自己已知版本拉取重段)。
    pub(crate) state_changes: watch::Receiver<u64>,

    /// 已转发给 client 的最新 finished seq;auto-next 监听它。
    pub(super) last_seen_finished_seq: AtomicU64,

    /// 包络离线计算的 in-flight 守卫(qualified id):开播 / 预排 / 收割多路
    /// 触发同曲时只解码一次。
    pub(crate) envelope_inflight: Mutex<rustc_hash::FxHashSet<String>>,

    /// 用户歌单库聚合态(原始数据唯一事实源 + curate 出口变换,见 [`crate::library`])。
    pub(super) library: crate::library::Library,

    /// 收藏(♥)persist 读改写 + canonical 推送的串行锁:让 toggle 与 connect 期 sync_favorites
    /// 互斥,防并发交错导致陈旧远端快照复活刚取消的收藏、或乐观收藏被整源桶替换清掉。
    /// 只护 persist 读写 + 推送,**不**跨远端网络调用(镜像 / fetch 都在锁外)。
    pub(crate) favorites_lock: tokio::sync::Mutex<()>,

    /// 上次「周期 position 刷新」落盘时刻;background_loop 按 `session_save` 节流。
    pub(crate) last_session_save: Mutex<Instant>,

    /// 在线播放音质(配置 `audio.playback_quality`,独立于下载音质)。
    pub(super) playback_quality: BitRate,

    /// Bytes prepared by built-in streaming openers before decoder handoff.
    pub(super) playback_prefetch_bytes: u64,

    /// 响度包络计算参数(配置 `audio.envelope`)。
    pub(crate) envelope_params: mineral_audio::EnvelopeParams,

    /// gapless 预排触发距曲终的剩余时间(ms,配置 `daemon.gapless_prefetch_ms`)。
    pub(super) gapless_prefetch_ms: u64,

    /// `p` 键的「回开头 vs 上一首」分界(ms,配置 `daemon.prev_restart_threshold_ms`)。
    pub(super) prev_restart_threshold_ms: u64,

    /// 长跑后台 task 的醒来间隔(ms,配置 `daemon.player_tick_ms`)。
    pub(super) player_tick_ms: u64,

    /// 会话「位置刷新」的节流间隔(配置 `daemon.session_save_secs`)。
    pub(crate) session_save: Duration,

    /// 系统媒体服务的播放进度上报间隔(ms,配置 `daemon.report_interval_ms`)。
    pub(super) media_report_interval_ms: u64,

    /// 系统媒体服务判定 seek 的位置跳变阈值(ms,配置 `daemon.seek_threshold_ms`)。
    pub(super) media_seek_threshold_ms: u64,

    /// 同步拦截 hook 软超时(配置 `script.hook_timeout_ms`)。
    pub(super) hook_timeout: Duration,

    /// `mineral.spawn` 并发上限(配置 `script.spawn_max_concurrent`;0 = 不限)。
    pub(super) spawn_max_concurrent: usize,

    /// 聚合收藏补 meta 后台任务的状态 + 节流旋钮(单飞闸 / 待办标志 / 并发参数,见 [`crate::favorites`])。
    pub(crate) backfill: crate::favorites::Backfill,
}

impl PlayerCore {
    /// 播放状态变更订阅端(会话发布器被唤醒后按自己已知版本拉取重段)。
    pub(crate) fn state_changes(&self) -> watch::Receiver<u64> {
        self.inner.state_changes.clone()
    }

    /// 版本门控同步:client 报已持版本号,仅落后部分以重段返回(语义见 [`PlayerSync`])。
    /// `known = 0` 时发送完整快照,启动 / tick 同一条路径。
    pub fn sync(&self, known: PlayerVersions) -> PlayerSync {
        self.inner.state.lock().sync(known)
    }

    /// 内部 AudioHandle 引用 — 给 [`crate::client::ClientHandle`] 转发 pause/seek
    /// 等无业务语义的低级操作。**不暴露**给 client trait;client 只能调 trait 方法。
    pub(crate) fn audio(&self) -> &AudioHandle {
        &self.inner.audio
    }

    /// 直通:scheduler 状态。
    pub fn task_snapshot(&self) -> Snapshot {
        self.inner.scheduler.snapshot()
    }

    /// 用户歌单库聚合态句柄(管线与脚本查询在 [`crate::library`] 消费)。
    pub(crate) fn library(&self) -> &crate::library::Library {
        &self.inner.library
    }

    /// 按 [`SourceKind`] 找对应的已注入 channel handle;无匹配返回 `None`。
    ///
    /// # Params:
    ///   - `source`: 目标音乐源。
    ///
    /// # Return:
    ///   命中的 channel handle 引用,无则 `None`。
    pub(crate) fn channel_for(&self, source: SourceKind) -> Option<&Arc<dyn MusicChannel>> {
        self.inner.channels.iter().find(|ch| ch.source() == source)
    }

    /// 已注入的全部音乐源(脚本 `library.playlists` 跨源聚合用)。
    pub(crate) fn channels(&self) -> &[Arc<dyn MusicChannel>] {
        &self.inner.channels
    }

    /// Returns the process-local playback provider registry.
    pub(crate) fn playback(&self) -> &PlaybackRegistry {
        &self.inner.playback
    }

    /// 音频本体缓存句柄引用(下载 / capture 编排在 [`crate::download`] 复用)。
    pub(crate) fn media_cache(&self) -> &Arc<MediaCache> {
        &self.inner.media_cache
    }

    /// 永久下载导出根目录；不可用时为 `None`。
    pub(crate) fn music_dir(&self) -> Option<&std::path::Path> {
        self.inner.music_dir.as_deref()
    }

    /// 打标队列投递端(下载 / 缓存落盘后投一曲;开关关闭为 null-object)。
    pub(crate) fn tagging(&self) -> &crate::tagging::TaggingQueue {
        &self.inner.tagging
    }

    /// 在线播放音质(配置 `audio.playback_quality`)。
    pub(crate) fn playback_quality(&self) -> BitRate {
        self.inner.playback_quality
    }

    /// Returns the encoded-byte prefetch target for playback openers.
    pub(crate) fn playback_prefetch_bytes(&self) -> u64 {
        self.inner.playback_prefetch_bytes
    }

    /// gapless 预排触发距曲终的剩余时间(ms,配置 `daemon.gapless_prefetch_ms`)。
    pub(crate) fn gapless_prefetch_ms(&self) -> u64 {
        self.inner.gapless_prefetch_ms
    }

    /// 同步拦截 hook 软超时(配置 `script.hook_timeout_ms`)。
    pub(crate) fn hook_timeout(&self) -> Duration {
        self.inner.hook_timeout
    }

    /// `mineral.spawn` 并发上限(配置 `script.spawn_max_concurrent`;0 = 不限)。
    pub(crate) fn spawn_max_concurrent(&self) -> usize {
        self.inner.spawn_max_concurrent
    }

    /// 系统媒体服务的播放进度上报间隔(ms,配置 `daemon.report_interval_ms`)。
    pub(crate) fn media_report_interval_ms(&self) -> u64 {
        self.inner.media_report_interval_ms
    }

    /// 系统媒体服务判定 seek 的位置跳变阈值(ms,配置 `daemon.seek_threshold_ms`)。
    pub(crate) fn media_seek_threshold_ms(&self) -> u64 {
        self.inner.media_seek_threshold_ms
    }

    /// 持久化句柄引用,供 [`crate::client::ClientHandle`] 查 love / 统计。
    ///
    /// # Return:
    ///   内部 [`ServerStore`] 句柄引用。
    pub(crate) fn persist(&self) -> &ServerStore {
        &self.inner.persist
    }

    /// 心跳用:是否已预排好下一首(gapless)。
    pub(crate) fn prefetched_ready(&self) -> bool {
        self.inner.state.lock().prefetch.is_armed()
    }

    /// 在锁内对播放状态跑一个闭包并返回其结果(gapless 编排在 [`crate::gapless`] 复用)。
    /// **不要**在闭包里再调本方法 —— `parking_lot::Mutex` 不可重入。
    ///
    /// # Params:
    ///   - `f`: 在 `&mut State` 上执行的闭包
    pub(crate) fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.inner.state.lock())
    }

    /// gapless 边界推进读已见 finished_seq(与 snapshot 的比对)。
    pub(crate) fn last_seen_finished_seq(&self) -> u64 {
        self.inner.last_seen_finished_seq.load(Ordering::Relaxed)
    }

    /// gapless 边界推进写已见 finished_seq(消费掉一次曲终事件)。
    pub(crate) fn set_last_seen_finished_seq(&self, seq: u64) {
        self.inner
            .last_seen_finished_seq
            .store(seq, Ordering::Relaxed);
    }

    /// 底层 audio handle 的播放状态快照(gapless 编排读 finished_seq / playing / 下完标记)。
    pub(crate) fn audio_snapshot(&self) -> mineral_audio::AudioSnapshot {
        self.inner.audio.snapshot()
    }

    /// 把 client 请求交给 scheduler；返回时不等待任务完成。
    pub fn submit_task(&self, kind: TaskKind, priority: Priority) {
        self.inner.scheduler.submit(kind, priority);
    }
}

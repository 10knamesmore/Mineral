//! [`ClientHandle`]:client 调 server 的指令面。

use mineral_audio::{AudioHandle, AudioSnapshot};
use mineral_model::MediaUrl;
use mineral_protocol::CancelFilter;
use mineral_task::{Priority, Scheduler, Snapshot, TaskEvent, TaskId, TaskKind};

/// Client → Server 指令面。`Clone` 廉价(内部全是 Arc handle)。
///
/// 所有方法签名都已 wire-friendly:入参 / 返回值要么是 serde-ready 的 model 类型,
/// 要么是本 crate 自有的 enum / id。**没有** 闭包、`TaskHandle`、`&Scheduler` 这类
/// 跨进程不可表达的形状。未来真要拆双进程时,把内部两个 Arc handle 替换成「unix
/// socket connection + serde 编码」即可,这层方法签名不动,所有 client 调用方零改动。
#[derive(Clone)]
pub struct ClientHandle {
    audio: AudioHandle,
    scheduler: Scheduler,
}

impl ClientHandle {
    pub(crate) fn new(audio: AudioHandle, scheduler: Scheduler) -> Self {
        Self { audio, scheduler }
    }

    // ---- 播放控制 ----

    /// 切到这个 URL,从头播。已有曲目会被立刻打断。
    pub fn play(&self, url: MediaUrl) {
        self.audio.play(url);
    }

    /// 暂停。
    pub fn pause(&self) {
        self.audio.pause();
    }

    /// 从暂停恢复。
    pub fn resume(&self) {
        self.audio.resume();
    }

    /// 停止当前曲目。
    pub fn stop(&self) {
        self.audio.stop();
    }

    /// 跳到绝对位置(ms)。语义 latest-wins:多次连按只生效最后一次。
    pub fn seek(&self, position_ms: u64) {
        self.audio.seek(position_ms);
    }

    /// 设置音量百分比(0..=100)。
    pub fn set_volume(&self, pct: u8) {
        self.audio.set_volume(pct);
    }

    /// 拉一次 audio 引擎当前快照(playing / position / duration / volume / 曲终序号 等)。
    pub fn audio_snapshot(&self) -> AudioSnapshot {
        self.audio.snapshot()
    }

    // ---- 任务调度 ----

    /// 提交一个任务。dedup / 优先级抢占由 scheduler 内部决定。返回任务 id ——
    /// 内部 await/cancel 形态(scheduler 的 `TaskHandle`)是不可跨进程的实现细节,
    /// 不漏到 client 面;真要监听结果走 [`Self::drain_task_events`]。
    pub fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskId {
        self.scheduler.submit(kind, priority).id
    }

    /// 按 [`CancelFilter`] 批量取消任务。enum 而不是闭包,可序列化跨进程。
    ///
    /// scheduler 内部 API 仍是 `cancel_where(closure)`;此方法做翻译。
    pub fn cancel_tasks(&self, filter: CancelFilter) {
        self.scheduler.cancel_where(move |k| filter.matches(k));
    }

    /// 拉走 server 端积攒的任务事件。client 主循环 tick 调一次。
    pub fn drain_task_events(&self) -> Vec<TaskEvent> {
        self.scheduler.drain_events()
    }

    /// 当前 scheduler 状态快照(running 数 / by_lane 拆分)。
    pub fn task_snapshot(&self) -> Snapshot {
        self.scheduler.snapshot()
    }
}

/// Client → Server 调用面的抽象。
///
/// 现有两个实现:
/// - [`ClientHandle`]:同进程,内部持 `Arc<AudioHandle>` + `Arc<Scheduler>`,直接转发
/// - `mineral_tui::remote::RemoteClient`:跨进程,内部走 unix socket
///
/// **实现方约定**:全部方法 sync。fire-and-forget 类(`play` / `pause` / 等)立即返回;
/// 返回值类(`audio_snapshot` / `submit_task` / `drain_task_events` / `task_snapshot`)
/// 允许阻塞等内部 I/O,但调用方期望 < 1ms(30fps 一帧 33ms 预算下绰绰有余)。
/// 出错时返回值类用「合理默认值」兜底(避免让上层处理一堆 Result)。
pub trait Client: Send + Sync {
    /// 切到这个 URL,从头播。
    fn play(&self, url: MediaUrl);

    /// 暂停。
    fn pause(&self);

    /// 从暂停恢复。
    fn resume(&self);

    /// 停止当前曲目。
    fn stop(&self);

    /// 跳到绝对位置(ms),latest-wins。
    fn seek(&self, position_ms: u64);

    /// 设置音量百分比(0..=100)。
    fn set_volume(&self, pct: u8);

    /// 拉一次音频快照。
    fn audio_snapshot(&self) -> AudioSnapshot;

    /// 提交一个任务,返回任务 id。
    fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskId;

    /// 按 [`CancelFilter`] 批量取消。
    fn cancel_tasks(&self, filter: CancelFilter);

    /// 拉走 server 端积攒的所有任务事件。
    fn drain_task_events(&self) -> Vec<TaskEvent>;

    /// 当前 scheduler 状态快照。
    fn task_snapshot(&self) -> Snapshot;
}

impl Client for ClientHandle {
    fn play(&self, url: MediaUrl) {
        Self::play(self, url);
    }
    fn pause(&self) {
        Self::pause(self);
    }
    fn resume(&self) {
        Self::resume(self);
    }
    fn stop(&self) {
        Self::stop(self);
    }
    fn seek(&self, position_ms: u64) {
        Self::seek(self, position_ms);
    }
    fn set_volume(&self, pct: u8) {
        Self::set_volume(self, pct);
    }
    fn audio_snapshot(&self) -> AudioSnapshot {
        Self::audio_snapshot(self)
    }
    fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskId {
        Self::submit_task(self, kind, priority)
    }
    fn cancel_tasks(&self, filter: CancelFilter) {
        Self::cancel_tasks(self, filter);
    }
    fn drain_task_events(&self) -> Vec<TaskEvent> {
        Self::drain_task_events(self)
    }
    fn task_snapshot(&self) -> Snapshot {
        Self::task_snapshot(self)
    }
}

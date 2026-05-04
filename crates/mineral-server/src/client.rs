//! [`ClientHandle`]:client 调 server 的指令面。

use mineral_audio::{AudioHandle, AudioSnapshot};
use mineral_model::MediaUrl;
use mineral_task::{Priority, Scheduler, Snapshot, TaskEvent, TaskHandle, TaskKind};

/// Client → Server 指令面。`Clone` 廉价(内部全是 Arc handle)。
///
/// 未来真要拆双进程时,把内部两个 Arc handle 替换成「unix socket connection +
/// serde 编码」即可,这层方法签名不动,所有 client 调用方零改动 ——
/// 这正是把它单独抽出来的目的。
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

    /// 提交一个任务。dedup / 优先级抢占由 scheduler 内部决定。
    pub fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskHandle {
        self.scheduler.submit(kind, priority)
    }

    /// 批量取消满足谓词的任务。
    pub fn cancel_tasks_where<F>(&self, pred: F)
    where
        F: Fn(&TaskKind) -> bool + Send + Sync,
    {
        self.scheduler.cancel_where(pred);
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

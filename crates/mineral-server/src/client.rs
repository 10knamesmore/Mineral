//! [`ClientHandle`]:client 调 server 的指令面。

use mineral_audio::{AudioHandle, AudioSnapshot};
use mineral_model::MediaUrl;
use mineral_task::{ChannelFetchKind, Priority, Scheduler, Snapshot, TaskEvent, TaskId, TaskKind};
use serde::{Deserialize, Serialize};

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

/// IPC-friendly 的批量取消条件。enum + Vec<tag>,可序列化。
///
/// 用 tag 而不是闭包是为了「跨进程能表达」——闭包过不了 wire,但「按种类砍一批」
/// 这种谓词足以覆盖现有所有 cancel 场景。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CancelFilter {
    /// 取消所有 [`TaskKind::ChannelFetch`] 任务,且其 `ChannelFetchKind` 命中给定 tag。
    /// 空 vec 等价 no-op。
    ChannelFetchKinds(Vec<ChannelFetchKindTag>),
}

impl CancelFilter {
    /// 给定一个具体 [`TaskKind`],判断本 filter 是否要砍。给 [`ClientHandle::cancel_tasks`]
    /// 内部翻译用,不过外面也可调试。
    #[must_use]
    pub fn matches(&self, kind: &TaskKind) -> bool {
        match (self, kind) {
            (Self::ChannelFetchKinds(tags), TaskKind::ChannelFetch(k)) => {
                tags.contains(&ChannelFetchKindTag::of(k))
            }
        }
    }
}

/// [`ChannelFetchKind`] 的 wire-friendly 标签。本身不带任何字段——只用于「按种类砍一批」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelFetchKindTag {
    /// 对应 [`ChannelFetchKind::MyPlaylists`]。
    MyPlaylists,
    /// 对应 [`ChannelFetchKind::LikedSongIds`]。
    LikedSongIds,
    /// 对应 [`ChannelFetchKind::PlaylistTracks`]。
    PlaylistTracks,
    /// 对应 [`ChannelFetchKind::SongUrl`]。
    SongUrl,
    /// 对应 [`ChannelFetchKind::Lyrics`]。
    Lyrics,
}

impl ChannelFetchKindTag {
    /// 取一个具体 [`ChannelFetchKind`] 的标签。
    #[must_use]
    pub fn of(kind: &ChannelFetchKind) -> Self {
        match kind {
            ChannelFetchKind::MyPlaylists { .. } => Self::MyPlaylists,
            ChannelFetchKind::LikedSongIds { .. } => Self::LikedSongIds,
            ChannelFetchKind::PlaylistTracks { .. } => Self::PlaylistTracks,
            ChannelFetchKind::SongUrl { .. } => Self::SongUrl,
            ChannelFetchKind::Lyrics { .. } => Self::Lyrics,
        }
    }
}

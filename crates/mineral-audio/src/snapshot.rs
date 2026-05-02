//! UI 周期拉取的播放状态快照。

/// 当前引擎状态的只读视图。`duration_ms == 0` 表示 decoder 尚未探出时长(或没在播)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioSnapshot {
    /// 是否正在出声(暂停 / 已结束 / 没有曲目时为 false)。
    pub playing: bool,

    /// 当前播放位置(ms)。
    pub position_ms: u64,

    /// 当前曲目时长(ms);decoder 没探出来时为 0。
    pub duration_ms: u64,

    /// 当前音量(0..=100)。
    pub volume_pct: u8,

    /// 单调递增的「曲终事件 latch」。每次 sink 自然播完一首歌(非 user-stop)engine
    /// 把它 +1。UI 维护 `last_seen_finished_seq`,看到增长就触发 advance —— 用 seq
    /// 而非 transient bool 是为了让 UI 在 tick 间隙也能可靠捕获边界,不会漏。
    pub track_finished_seq: u64,
}

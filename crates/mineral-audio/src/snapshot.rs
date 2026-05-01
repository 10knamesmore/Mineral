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
}

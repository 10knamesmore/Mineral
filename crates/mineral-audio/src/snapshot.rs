//! UI 周期拉取的播放状态快照。

use serde::{Deserialize, Serialize};

/// 音频输出后端的当前形态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioBackend {
    /// 正常:拿到默认输出设备,真出声。
    #[default]
    Device,

    /// 降级:无可用音频设备,引擎空跑——命令被接受但不发声。
    Null,
}

/// 当前引擎状态的只读视图。`duration_ms == 0` 表示 decoder 尚未探出时长(或没在播)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

    /// 音频输出后端形态。`Null` 表示无设备降级(命令被接受但不发声),
    /// client(CLI status / TUI 顶栏)据此提示用户。
    pub backend: AudioBackend,

    /// 当前曲目的**远端字节是否已下完**(仅 capture 播放有意义;本地 / 非 capture 恒 false)。
    /// 一完成即为 true,不必等播放结束——上层据此把 capture 文件 harvest 进缓存。切歌后
    /// 在新曲下完前回落为 false。
    pub download_complete: bool,
}

//! UI 周期拉取的播放状态快照。

use mineral_common::revision::Revision;
use serde::{Deserialize, Serialize};

use crate::bps::Bps;

/// 音频输出后端的当前形态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioBackend {
    /// 正常:已打开系统输出流。
    #[default]
    Device,

    /// 没有可用输出流，或启动时强制禁用设备输出。
    Null,
}

/// 当前引擎状态的只读视图。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSnapshot {
    /// 当前已打开的设备与实际流配置；没有可用输出流时为 `None`。
    pub output: Option<std::sync::Arc<crate::AudioOutput>>,

    /// 当前曲目已启用播放；暂停、结束、无曲目或无设备时为 false。
    pub playing: bool,

    /// 播放已启用但输出回调正在等待 PCM，期间位置不应外推。
    pub buffering: bool,

    /// 当前播放位置(ms)。
    pub position_ms: u64,

    /// 当前曲目时长(ms);`None` = decoder 没探出来(分片容器流式打开探不出总长)或没在播。
    /// 消费方需要总长时应显式回落别的口径(如歌曲元数据),不要默认吃 0。
    pub duration_ms: Option<u64>,

    /// 当前音量(0..=100)。
    pub volume_pct: u8,

    /// 每次输出回调消费完一首曲目的 PCM 后递增；失败、取消和停止不计入。
    /// 控制线程读取序号判断曲终，不依赖队列长度或瞬时布尔值。
    pub track_finished_seq: u64,

    /// 音频输出后端形态。`Null` 表示当前没有可用输出流,
    /// client(CLI status / TUI 顶栏)据此提示用户。
    pub backend: AudioBackend,

    /// 当前曲目已缓冲到的比例。本地 / 已完整缓存恒满;远端流式播放时由 stream-download
    /// 的下载进度回调驱动,随已下字节占总字节推进。总长未知(无 Content-Length)时维持
    /// 零,直到整段下完跳满。切歌瞬间回落零。
    pub buffered_bps: Bps,

    /// 当前曲目身份令牌:每次起播 / 无缝边界轮转 +1。上层认它的**值变化**判定换曲——
    /// 比单调 `track_finished_seq` 的边沿更稳:漏一个 tick 也不会丢轨(值对不上就重新对齐)。
    pub current_track_token: Revision,

    /// 已预排的下一曲时长(ms);`None` = 未预排 / decoder 未探出。
    pub next_duration_ms: Option<u64>,

    /// 已预排的下一曲缓冲比例;未预排恒零。
    pub next_buffered_bps: Bps,

    /// 下一曲已准备出可供输出回调消费的 PCM。
    pub next_ready: bool,

    /// 当前曲目采样率(Hz),由 decoder 解出后写入;无缝边界轮转时切到下一曲的值。
    /// 没在播 / 未探出为 0。transport 据此在 fmt 段显采样率(如 `44.1kHz`)。
    pub sample_rate_hz: u32,
}

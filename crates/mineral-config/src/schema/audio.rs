//! 音频段(音量 / 后端 / 播放音质 / 引擎内参)。
//!
//! [`BackendKind`] 与音频层的后端模式语义对齐,但保持 config 与音频 crate 解耦——
//! client 接线处做 `BackendKind → 音频后端模式` 映射,本枚举不依赖音频 crate。

use mineral_config_macros::config_section;
use mineral_model::BitRate;
use serde::Deserialize;

/// 音频段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct AudioConfig {
    /// 初始音量百分比 0-100。
    volume: u8,

    /// 后端选择。
    backend: BackendKind,

    /// 在线播放音质(独立于下载音质)。
    playback_quality: BitRate,

    /// 音频引擎主循环 tick 间隔(毫秒)。
    engine_tick_ms: u64,

    /// 流式播放起播前预拉的字节数。
    prefetch_bytes: u64,

    /// FFT tap 环形缓冲容量(采样点)。**外键**:须 ≥ 2 × `tui.spectrum.fft_size`。
    tap_capacity: usize,
}

/// 音频后端选择。不依赖音频 crate;接线处映射到具体后端模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum BackendKind {
    /// 自动探测,无设备降级 Null(默认)。
    Auto,

    /// 强制空跑(无声卡)。
    Null,
}

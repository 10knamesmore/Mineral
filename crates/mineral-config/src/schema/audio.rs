//! 音频段(音量 / 后端 / 播放音质 / 引擎内参)。
//!
//! [`BackendKind`] 与音频层的后端模式语义对齐,但保持 config 与音频 crate 解耦——
//! client 接线处做 `BackendKind → 音频后端模式` 映射,本枚举不依赖音频 crate。

use mineral_config_macros::{config_section, lua_enum};
use mineral_model::BitRate;
use serde::Deserialize;

use super::envelope::EnvelopeConfig;

/// 音频配置
#[config_section]
pub struct AudioConfig {
    /// 启动音量 0-100，每次启动重置
    volume: u8,

    /// 音频后端，MINERAL_AUDIO_NULL 优先
    backend: BackendKind,

    /// 在线播放音质
    playback_quality: BitRate,

    /// 音频引擎轮询间隔
    engine_tick_ms: u64,

    /// 起播预取字节数
    prefetch_bytes: u64,

    /// 采样缓冲点数，须 ≥ 2 × tui.spectrum.fft_size
    tap_capacity: usize,

    /// 响度包络计算
    envelope: EnvelopeConfig,
}

/// 音频后端
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum BackendKind {
    /// 自动探测，无设备则空跑
    Auto,

    /// 无声输出
    Null,
}

//! 进度条波形的开关、高度映射与入场动画。
//!
//! 普通进度条与波形共用 `progress` 配色；场景化开关由脚本 override `enabled` 实现。

use mineral_config_macros::config_section;

/// 进度条波形配置。
#[config_section]
pub struct WaveformConfig {
    /// 显示波形；包络未就绪用普通进度条
    enabled: bool,

    /// 波形高度 gamma；1 为线性
    contrast: f32,

    /// 波形入场动画
    reveal: RevealConfig,
}

/// 波形入场动画
#[config_section]
pub struct RevealConfig {
    /// 动画时长
    duration_ms: u32,

    /// 横扫时长占比 0-1，余下为纵向生长
    sweep_ratio: f32,

    /// 前沿提亮强度 0-1
    glow: f32,
}

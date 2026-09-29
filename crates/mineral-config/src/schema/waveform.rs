//! 进度条波形段(挂在 `TuiConfig` 下):transport 进度条化身全曲振幅波形。
//!
//! 只承载两个正交机制开关。「全屏才展开波形」等场景化行为**不进**核心配置——
//! 由用户脚本 observe terminal 态后 override `enabled` 实现(见配置文档 recipe)。

use mineral_config_macros::config_section;

/// 进度条波形配置。
#[config_section]
pub struct WaveformConfig {
    /// 显示波形；包络未就绪用普通进度条
    enabled: bool,

    /// 已播放段使用封面色
    cover_color: bool,

    /// 波形高度 gamma；1 为线性
    contrast: f32,

    /// 播放头软边半径（列）
    edge_radius: usize,

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

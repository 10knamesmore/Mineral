//! 进度条波形段(挂在 `TuiConfig` 下):transport 进度条化身全曲振幅波形。
//!
//! 只承载两个正交机制开关。「全屏才展开波形」等场景化行为**不进**核心配置——
//! 由用户脚本 observe terminal 态后 override `enabled` 实现(见配置文档 recipe)。

use mineral_config_macros::config_section;

/// 进度条波形配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct WaveformConfig {
    /// 进度条是否化身全曲振幅波形;包络未就绪(未缓存 / 流播中)自动回落普通进度条。
    enabled: bool,

    /// 已播放段是否吃封面取色;关闭时用主题 accent。
    cover_color: bool,
}

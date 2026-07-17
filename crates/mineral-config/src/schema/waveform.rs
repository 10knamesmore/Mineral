//! 进度条波形段(挂在 `TuiConfig` 下):transport 进度条化身全曲振幅波形。
//!
//! 只承载两个正交机制开关。「全屏才展开波形」等场景化行为**不进**核心配置——
//! 由用户脚本 observe terminal 态后 override `enabled` 实现(见配置文档 recipe)。

use mineral_config_macros::config_section;

/// 进度条波形配置。
#[config_section]
pub struct WaveformConfig {
    /// 进度条是否化身全曲振幅波形;包络未就绪(未缓存 / 流播中)自动回落普通进度条。
    enabled: bool,

    /// 已播放段是否吃封面取色;关闭时用主题 accent。
    cover_color: bool,

    /// 响度 → 条高的对比 gamma(渲染层幂映射 `v^contrast`,不改包络数据,改了即时生效):
    /// 1 = 线性,越大安静段压得越低、起伏越明显。
    contrast: f32,

    /// 播放头软边半径(列):播放头前后各此数列在已播色与轨道色之间插值,边界雾化;
    /// 0 = 硬边(无播放头高亮,已播渐变的生长边缘即 seek 位置)。
    edge_radius: usize,
}

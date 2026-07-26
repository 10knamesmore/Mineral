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

    /// 包络到达时波形的入场揭示动画。
    reveal: RevealConfig,
}

/// 波形入场揭示动画配置(挂在 `WaveformConfig` 下):包络到达时波形不瞬变,而是
/// 揭示边从左扫到右、每列在被扫到时从底部长到目标高度,两个方向合成一条对角线。
///
/// 未揭示的列维持普通进度条形态(`━●─`),故整个过程读作「波形把中线推走」而非凭空出现。
#[config_section]
pub struct RevealConfig {
    /// 动画总时长(毫秒);0 = 一帧到位(等同关闭动画,包络到达即整条波形就位)。
    duration_ms: u32,

    /// 横扫占总时长的比例 0-1:余下部分是单列从底部长到目标高度的生长时长。
    /// 1 = 纯左→右擦除(列一到位就是满高),0 = 全条同时从底部抬升(无横向次序)。
    sweep_ratio: f32,

    /// 揭示边前沿的提亮强度 0-1:正在生长的列朝主题 `text` 混色,越接近到位越回落本色,
    /// 像一道光把波形画出来。0 = 无亮边(只剩对角揭示)。
    glow: f32,
}

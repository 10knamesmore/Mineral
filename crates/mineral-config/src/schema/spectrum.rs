//! 频谱面板段(挂在 `TuiConfig` 下):渲染风格 + 共用旋钮 + per-style 子表。
//!
//! 仅承载强类型旋钮;真正读取在 client 接线处。频谱结构性常量(分辨率契约 / 首帧占位 /
//! 色场采样精度)与 DSP 核心(FFT/频率/动态范围)**不**在此——它们是算法内参。
//!
//! 顶层放跨风格共用的旋钮:FFT/dB 标定喂条形语义的风格(bars/waterfall/terrain),
//! ADSR 包络驱动 bars/terrain 的条高,配色态机(hue/封面色场)全风格通吃。
//! 只被单一风格消费的旋钮进对应子表(`bars` / `scope` / `waterfall` / `terrain`)。
//!
//! 所有时长旋钮均为**毫秒**,运行时按 `animation.frame_tick_ms` 折算成拍数——
//! 与帧率解耦,改帧率不改手感。条高动态沿用效果器 ADSR 模型:attack(起音,上升)、
//! decay(衰减,播放中向更低目标回落 = 余韵)、release(释音,暂停时落向 0);
//! sustain 即 FFT 实时值本身,无旋钮。

use mineral_config_macros::config_section;
use serde::Deserialize;

/// 频谱面板配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct SpectrumConfig {
    /// 渲染风格。
    style: SpectrumStyle,

    /// FFT 窗大小(样本)。**外键**:`audio.tap_capacity` 须 ≥ 2 × 此值。
    fft_size: usize,

    /// 频率轴下界(Hz)。
    f_min: f32,

    /// 频率轴上界(Hz);超过奈奎斯特时取奈奎斯特。
    f_max: f32,

    /// 频率轴对数化程度(0 线性 ..= 1 纯对数)。
    log_axis_blend: f32,

    /// dB 标定下界(低于此条高为 0)。
    db_floor: f32,

    /// dB 标定上界(高于此满高);必须 > `db_floor`。
    db_ceil: f32,

    /// 频带统计中峰值的占比(0 纯均值 ..= 1 纯峰值)。
    peak_mix: f32,

    /// 是否启用色相缓慢漂移。
    hue_rotate: bool,

    /// 任何状态下条的最小高度(1/8 字符单位)。
    baseline_min: u16,

    /// 起音(attack):条高**上升**到位 90% 所需毫秒。越小越跟手(鼓点立即顶上),
    /// 越大越钝;≤ 帧间隔时退化为瞬时。
    attack_ms: u32,

    /// 衰减(decay):播放中条高**向更低目标回落** 90% 所需毫秒(余韵滑落时长)。
    /// 动画感主要来自这里——比 `attack_ms` 大才有"快攻慢放"的运动轨迹。
    decay_ms: u32,

    /// 释音(release):暂停/无信号时条高落向 0(止于 `baseline_min`)90% 所需毫秒。
    release_ms: u32,

    /// 色相旋转一整圈(360°)的毫秒数。
    hue_cycle_ms: u32,

    /// 封面就绪后从当前配色缓动到封面色场的过渡毫秒数。
    cover_fade_ms: u32,

    /// 色场纵向采样偏移(‰):顶端比底端沿色带多偏向高频多少。
    cover_vshift_permille: u32,

    /// bars 风格参数;`style = "bars"` 时生效。
    bars: BarsConfig,

    /// scope 风格参数;`style = "scope"` 时生效。
    scope: ScopeConfig,

    /// waterfall 风格参数;`style = "waterfall"` 时生效。
    waterfall: WaterfallConfig,

    /// terrain 风格参数;`style = "terrain"` 时生效。
    terrain: TerrainConfig,
}

/// 频谱渲染风格。不依赖渲染 crate;接线处映射到具体画法。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SpectrumStyle {
    /// 频谱柱(默认):FFT 条高 + peak cap / trail 装饰。
    Bars,

    /// 示波器:时域 min/max 包络滚动(右新左旧),不走 FFT。
    Scope,

    /// 频谱历史瀑布:每帧 FFT 条高推入历史,x=频率 y=时间,`▀` 半块
    /// 一字符行装两帧、幅度→热力色。
    Waterfall,

    /// 山脊地形:每层一帧 ADSR 平滑后的历史频谱轮廓,前景遮挡后景。
    Terrain,
}

/// bars 风格参数(挂在 `SpectrumConfig` 下):peak cap / trail 装饰 + 弹簧物理。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct BarsConfig {
    /// 是否显示 peak cap(`▔` 浮在条顶)。
    show_peak_cap: bool,

    /// 是否显示 trail(peak 与 bar 间余韵 fade)。
    show_trail: bool,

    /// 是否启用 peak 弹簧物理(过冲 + 阻尼回弹)。
    spring_peak: bool,

    /// 新 peak 跟涨后在原位悬停的毫秒数。
    peak_hold_ms: u32,

    /// peak 悬停结束后,从满高(64 单位)落到 0 的满程毫秒数。
    peak_fall_ms: u32,

    /// 弹簧刚度(每 tick `force += stiffness × (target - pos)`)。
    /// **注**:弹簧是无量纲系数制,与 `animation.frame_tick_ms` 耦合——改帧率会改弹簧手感。
    spring_stiffness: f32,

    /// 弹簧阻尼(每 tick `force -= damping × velocity`)。同上,与帧率耦合。
    spring_damping: f32,
}

/// scope 风格参数(挂在 `SpectrumConfig` 下)。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct ScopeConfig {
    /// 每根包络列聚合的音频时长(毫秒)= 滚动速度:越小滚得越快、可见时间窗越短。
    column_ms: u32,
}

/// waterfall 风格参数(挂在 `SpectrumConfig` 下)。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct WaterfallConfig {
    /// 推行间隔(毫秒)。`▀` 半块一字符行装两帧 = 每半格 `push_ms / 2` 毫秒;
    /// 越小流速越快、可见历史窗越短。
    push_ms: u32,

    /// 幅度→热力色的对比 gamma(`色档 = (幅度/RES)^contrast × RES`,端点不动、单调)。
    /// 1 = 线性;> 1 压暗噪底、只留强峰(音高线更突出);< 1 抬亮弱能量
    /// (泛音/弱谐波浮现,代价是噪声也起)。非有限 / 非正值按 1 处理。
    contrast: f32,
}

/// terrain 风格参数(挂在 `SpectrumConfig` 下)。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct TerrainConfig {
    /// 推层间隔(毫秒)。层数 × 此值 = 地形时间纵深。
    push_ms: u32,

    /// 历史层数。层距 = 可用纵深 / 层数,层数多了单层太挤。
    layers: usize,

    /// 轮廓振幅占面板高的比例(0..=1)。须明显小于 1,否则层间交叠过深互相淹没。
    amplitude: f32,

    /// 远层亮度保底(0..=1):越旧的历史层亮度衰减到此为止,不再隐入衬底。
    /// 0 = 远层淡到全隐;1 = 全层等亮、无纵深;抬高 = 后景更清晰、纵深更浅。
    fade_floor: f32,
}

#[cfg(test)]
mod tests {
    use crate::schema::{Config, SpectrumStyle};

    /// 默认 style = bars 与 per-style 子表默认值(default.lua 是唯一真相源,
    /// 这里锁枚举落型与子表结构的对应关系)。
    #[test]
    fn style_defaults_parse_to_enums() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let spectrum = cfg.tui().spectrum();
        assert_eq!(spectrum.style(), &SpectrumStyle::Bars);
        assert!(*spectrum.bars().show_peak_cap());
        assert_eq!(*spectrum.scope().column_ms(), 16);
        assert_eq!(*spectrum.waterfall().push_ms(), 64);
        assert!((*spectrum.waterfall().contrast() - 1.4).abs() < f32::EPSILON);
        assert_eq!(*spectrum.terrain().layers(), 8);
        assert!((*spectrum.terrain().fade_floor() - 0.30).abs() < f32::EPSILON);
        Ok(())
    }
}

//! 频谱面板段(挂在 `TuiConfig` 下):观感开关 + 平滑/衰减 + peak 物理。
//!
//! 仅承载强类型旋钮;真正读取在 client 接线处。频谱结构性常量(分辨率契约 / 首帧占位 /
//! 色场采样精度)与 DSP 核心(FFT/频率/动态范围)**不**在此——它们是算法内参。

use serde::Deserialize;

/// 频谱面板配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SpectrumConfig {
    /// 是否显示 peak cap(`▔` 浮在条顶)。
    show_peak_cap: bool,

    /// 是否显示 trail(peak 与 bar 间余韵 fade)。
    show_trail: bool,

    /// 是否启用色相缓慢漂移。
    hue_rotate: bool,

    /// 是否启用 peak 弹簧物理(过冲 + 阻尼回弹)。
    spring_peak: bool,

    /// 任何状态下条的最小高度(1/8 字符单位)。
    baseline_min: u16,

    /// 上升平滑(attack)旧值权重。
    attack_old: u32,

    /// 上升平滑(attack)新值权重。
    attack_new: u32,

    /// 静默/暂停时条高每 tick 衰减除数(指数项)。
    decay_div: u16,

    /// 衰减常数项(叠加在指数项上)。
    decay_step: u16,

    /// 新 peak 跟涨后在原位 hold 多少 tick 才下落。
    peak_hold_ticks: u8,

    /// hold 结束后每 tick peak 下落多少单位。
    peak_fall_per_tick: u16,

    /// 色相旋转一整圈(360°)的 tick 数。
    hue_cycle_ticks: u32,

    /// 封面就绪后从当前配色缓动到封面色场的过渡时长(tick)。
    cover_fade_ticks: u32,

    /// 色场纵向采样偏移(‰):顶端比底端沿色带多偏向高频多少。
    cover_vshift_permille: u32,

    /// 弹簧刚度(每 tick `force += stiffness × (target - pos)`)。
    spring_stiffness: f32,

    /// 弹簧阻尼(每 tick `force -= damping × velocity`)。
    spring_damping: f32,
}

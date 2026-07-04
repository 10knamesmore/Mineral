//! 频谱面板段(挂在 `TuiConfig` 下):观感开关 + ADSR 包络 + peak 物理。
//!
//! 仅承载强类型旋钮;真正读取在 client 接线处。频谱结构性常量(分辨率契约 / 首帧占位 /
//! 色场采样精度)与 DSP 核心(FFT/频率/动态范围)**不**在此——它们是算法内参。
//!
//! 所有时长旋钮均为**毫秒**,运行时按 `animation.frame_tick_ms` 折算成拍数——
//! 与帧率解耦,改帧率不改手感。条高动态沿用效果器 ADSR 模型:attack(起音,上升)、
//! decay(衰减,播放中向更低目标回落 = 余韵)、release(释音,暂停时落向 0);
//! sustain 即 FFT 实时值本身,无旋钮。

use mineral_config_macros::config_section;

/// 频谱面板配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct SpectrumConfig {
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

    /// 起音(attack):条高**上升**到位 90% 所需毫秒。越小越跟手(鼓点立即顶上),
    /// 越大越钝;≤ 帧间隔时退化为瞬时。
    attack_ms: u32,

    /// 衰减(decay):播放中条高**向更低目标回落** 90% 所需毫秒(余韵滑落时长)。
    /// 动画感主要来自这里——比 `attack_ms` 大才有"快攻慢放"的运动轨迹。
    decay_ms: u32,

    /// 释音(release):暂停/无信号时条高落向 0(止于 `baseline_min`)90% 所需毫秒。
    release_ms: u32,

    /// 新 peak 跟涨后在原位悬停的毫秒数。
    peak_hold_ms: u32,

    /// peak 悬停结束后,从满高(64 单位)落到 0 的满程毫秒数。
    peak_fall_ms: u32,

    /// 色相旋转一整圈(360°)的毫秒数。
    hue_cycle_ms: u32,

    /// 封面就绪后从当前配色缓动到封面色场的过渡毫秒数。
    cover_fade_ms: u32,

    /// 色场纵向采样偏移(‰):顶端比底端沿色带多偏向高频多少。
    cover_vshift_permille: u32,

    /// 弹簧刚度(每 tick `force += stiffness × (target - pos)`)。
    /// **注**:弹簧是无量纲系数制,与 `animation.frame_tick_ms` 耦合——改帧率会改弹簧手感。
    spring_stiffness: f32,

    /// 弹簧阻尼(每 tick `force -= damping × velocity`)。同上,与帧率耦合。
    spring_damping: f32,
}

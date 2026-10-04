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

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

/// 频谱面板配置。
#[config_section]
#[derive(PartialEq)]
pub struct SpectrumConfig {
    /// 渲染风格。
    style: SpectrumStyle,

    /// FFT 窗采样点数；audio.tap_capacity 须 ≥ 此值两倍
    fft_size: usize,

    /// 频率下界（Hz）
    f_min: f32,

    /// 频率上界（Hz），不超过采样率一半
    f_max: f32,

    /// 频率轴比例，0 线性、1 对数
    log_axis_blend: f32,

    /// 音量下界（dB），须小于 db_ceil
    db_floor: f32,

    /// 音量上界（dB），须大于 db_floor
    db_ceil: f32,

    /// 峰值占比，0 均值、1 峰值
    peak_mix: f32,

    /// 无封面色时轮转色相
    hue_rotate: bool,

    /// 柱高下限（1/8 字符，0-64）
    baseline_min: u16,

    /// 柱高上升至目标 90% 的时长
    attack_ms: u32,

    /// 播放中柱高回落 90% 的时长
    decay_ms: u32,

    /// 暂停后柱高回落 90% 的时长，仅 bars 生效
    release_ms: u32,

    /// 色相轮转周期
    hue_cycle_ms: u32,

    /// 封面配色过渡时长
    cover_fade_ms: u32,

    /// 色带纵向偏移 0-1000
    cover_vshift_permille: u32,

    /// 点阵与背景最小亮度差 0-1
    dot_bg_contrast: f32,

    /// 柱形频谱
    bars: BarsConfig,

    /// 波形示波器
    scope: ScopeConfig,

    /// 瀑布频谱
    waterfall: WaterfallConfig,

    /// 山脊频谱
    terrain: TerrainConfig,
}

/// 频谱样式
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SpectrumStyle {
    /// 柱形频谱
    Bars,

    /// 波形示波器
    Scope,

    /// 瀑布频谱
    Waterfall,

    /// 山脊频谱
    Terrain,
}

/// 柱形频谱
#[config_section]
#[derive(PartialEq)]
pub struct BarsConfig {
    /// 显示峰值横线
    show_peak_cap: bool,

    /// 显示峰值拖尾
    show_trail: bool,

    /// 启用峰值弹簧
    spring_peak: bool,

    /// 峰值悬停时长
    peak_hold_ms: u32,

    /// 峰值从满高落到底的时长
    peak_fall_ms: u32,

    /// 弹簧刚度，效果随帧间隔变化
    spring_stiffness: f32,

    /// 弹簧阻尼，效果随帧间隔变化
    spring_damping: f32,
}

/// 波形示波器
#[config_section]
#[derive(PartialEq)]
pub struct ScopeConfig {
    /// 每列音频时长
    column_ms: u32,
}

/// 瀑布频谱
#[config_section]
#[derive(PartialEq)]
pub struct WaterfallConfig {
    /// 推行间隔，半格为此值一半
    push_ms: u32,

    /// 热力色 gamma；1 为线性，须为有限正数
    contrast: f32,
}

/// 山脊频谱
#[config_section]
#[derive(PartialEq)]
pub struct TerrainConfig {
    /// 推层间隔
    push_ms: u32,

    /// 历史层数
    layers: usize,

    /// 振幅占面板高度比例 0-1
    amplitude: f32,

    /// 远层亮度下限 0-1
    fade_floor: f32,
}

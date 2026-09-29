//! 氛围背景段(挂在 `TuiConfig` 下):全屏沉浸页的调色板渐变场。

use mineral_config_macros::config_section;

/// 氛围背景配置。
#[config_section]
pub struct AmbientConfig {
    /// 是否启用
    enabled: bool,

    /// 渐变场浓度 0-1:0 = 纯主题底色
    intensity: f32,

    /// 色斑半径（屏幕相对坐标）
    sigma: f32,

    /// 切歌 / 封面取色就绪时调色板渐变到新封面的时长
    fade_ms: u32,

    /// 边缘暗角
    vignette: VignetteConfig,

    /// 锚点漂移
    drift: DriftConfig,

    /// 颜色轮转
    rotate: RotateConfig,

    /// 响度跳动
    pulse: PulseConfig,

    /// 渐变场锚点，数组整体替换
    anchors: Vec<AnchorConfig>,
}

/// 边缘暗角
#[config_section]
pub struct VignetteConfig {
    /// 暗角强度 0-1
    strength: f32,

    /// 起始半径(到屏心的相对距离,0-1)。
    inner: f32,

    /// 满强半径(到屏心的相对距离,0-1)。
    outer: f32,
}

/// 锚点漂移
#[config_section]
pub struct DriftConfig {
    /// 是否漂移
    enabled: bool,

    /// 漂移速率倍率
    speed: f32,

    /// 锚点摆幅占屏幕的百分比
    sway_pct: f32,
}

/// 锚点颜色往返轮转
#[config_section]
pub struct RotateConfig {
    /// 是否轮转
    enabled: bool,

    /// 整个往返周期的时长
    cycle_secs: f32,
}

/// 随音乐响度调节背景
#[config_section]
pub struct PulseConfig {
    /// 是否启用
    enabled: bool,

    /// 各调制目标的深度
    depth: PulseDepthConfig,

    /// 主包络 attack
    attack_ms: u32,

    /// 主包络 release
    release_ms: u32,

    /// 归一跟踪窗口
    gain_window_secs: f32,

    /// 低通滤波截止频率
    bass_cutoff_hz: f32,

    /// gamma 感知曲线
    gamma: f32,

    /// punch 瞬态通道
    punch: PunchConfig,
}

/// 响度调制深度；0 为不调制
#[config_section]
pub struct PulseDepthConfig {
    /// 场浓度增量 0-1
    intensity: f32,

    /// 色斑半径膨胀比例 0-1
    sigma: f32,

    /// 亮度变化 0-1
    brightness: f32,

    /// 暗角减弱比例 0-1
    vignette: f32,
}

/// 鼓点瞬态包络
#[config_section]
pub struct PunchConfig {
    /// 瞬态混入强度 0-1；0 为关闭
    gain: f32,

    /// 瞬态释放时长
    release_ms: u32,
}

/// 渐变场锚点
#[config_section]
pub struct AnchorConfig {
    /// 横坐标，屏幕相对值 0-1
    x: f32,

    /// 纵坐标，屏幕相对值 0-1
    y: f32,

    /// 色带位置，0 最暗、1000 最亮
    pos: u32,

    /// 横向角速度（弧度/秒），乘 drift.speed
    speed_x: f32,

    /// 纵向角速度（弧度/秒），乘 drift.speed
    speed_y: f32,

    /// 横向初始相位（弧度）
    phase_x: f32,

    /// 纵向初始相位（弧度）
    phase_y: f32,
}

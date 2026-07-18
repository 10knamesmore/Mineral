//! 氛围背景段(挂在 `TuiConfig` 下):全屏沉浸页的调色板渐变场。

use mineral_config_macros::config_section;

/// 氛围背景配置。
#[config_section]
pub struct AmbientConfig {
    /// 是否启用(全屏沉浸页整屏铺当前封面调色板驱动的氛围渐变;关闭经渐变平滑淡出)。
    enabled: bool,

    /// 渐变场浓度 0-1:0 = 纯主题底色,越大封面色越浓;过大前景文字对比度下降。
    intensity: f32,

    /// 锚点高斯权重的半径 σ(屏幕相对坐标):越小色斑越聚拢,越大场越均匀。
    sigma: f32,

    /// 切歌 / 封面取色就绪时调色板渐变到新封面的时长,毫秒。
    fade_ms: u32,

    /// 边缘暗角(向主题底色收敛,保歌词 / 播控可读)。
    vignette: VignetteConfig,

    /// 锚点漂移(渐变场的缓慢流动)。
    drift: DriftConfig,

    /// 颜色轮转(各锚点颜色沿封面色带循环流动)。
    rotate: RotateConfig,

    /// 渐变场锚点表(数组整体替换):每锚点一个色斑,颜色取封面色带 `pos` 位置。
    anchors: Vec<AnchorConfig>,
}

/// 氛围背景的边缘暗角参数(挂在 `AmbientConfig` 下)。
#[config_section]
pub struct VignetteConfig {
    /// 暗角强度 0-1:屏幕边缘向主题底色收敛的程度;0 = 关闭。
    strength: f32,

    /// 起始半径(到屏心的相对距离,0-1;此距离以内不叠暗角)。
    inner: f32,

    /// 满强半径(到屏心的相对距离;到此距离暗角达 `strength`,屏角距离 ≈ 0.71)。
    outer: f32,
}

/// 氛围背景的锚点漂移参数(挂在 `AmbientConfig` 下)。
#[config_section]
pub struct DriftConfig {
    /// 是否漂移;关闭则场静止(仅切歌时颜色过渡)。
    enabled: bool,

    /// 漂移速率倍率:1 = 各锚点自带速率的原速,越大流动越快。
    speed: f32,

    /// 锚点摆幅,屏幕尺寸的 %:锚点绕锚位正弦游走的半径。
    sway_pct: f32,
}

/// 氛围背景的颜色轮转参数(挂在 `AmbientConfig` 下):各锚点的色带采样位随时间
/// 沿「暗 → 亮 → 暗」三角波往返,封面色在色斑间缓慢流动。
#[config_section]
pub struct RotateConfig {
    /// 是否轮转;关闭则各锚点颜色钉在其 `pos` 位置。
    enabled: bool,

    /// 一整个往返周期的时长,秒;越小颜色流动越快。
    cycle_secs: f32,
}

/// 渐变场单个锚点(挂在 `AmbientConfig.anchors` 数组)。
#[config_section]
pub struct AnchorConfig {
    /// 锚位横坐标(屏幕相对 0-1)。
    x: f32,

    /// 锚位纵坐标(屏幕相对 0-1)。
    y: f32,

    /// 绑定的色带采样位置(‰,0-1000;0 = 色板最暗色,1000 = 最亮色)。
    pos: u32,

    /// 横轴漂移角速度(弧度/秒,乘 `drift.speed` 后生效)。
    speed_x: f32,

    /// 纵轴漂移角速度(弧度/秒;与横轴不同才游走出非正圆轨迹)。
    speed_y: f32,

    /// 横轴初始相位(弧度;各锚点错开,静止帧也不对称)。
    phase_x: f32,

    /// 纵轴初始相位(弧度)。
    phase_y: f32,
}

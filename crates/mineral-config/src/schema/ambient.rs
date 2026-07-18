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

    /// 响度跳动(场浓度随播放响度呼吸)。
    pulse: PulseConfig,

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

/// 氛围背景的响度跳动参数(挂在 `AmbientConfig` 下):正在播放的音频每帧过低通
/// 滤波(可选)算 RMS 响度,在近期峰值与谷值间归一(把压限压扁的动态重新撑开),
/// 过 gamma 感知曲线后由主包络(attack/release)与 punch 瞬态通道合成,按 `depth`
/// 叠加到场浓度上——音乐越响封面色越浓,随鼓点呼吸。
#[config_section]
pub struct PulseConfig {
    /// 是否启用;关闭则场完全不随响度变化。
    enabled: bool,

    /// 各调制目标的深度(响度作用到哪些旋钮、作用多深)。
    depth: PulseDepthConfig,

    /// 主包络 attack 时间常数,毫秒:越小越跟得上鼓点瞬态,过小会抖。
    attack_ms: u32,

    /// 主包络 release 时间常数,毫秒:越大鼓点之间衰减越平滑,过小会闪。
    release_ms: u32,

    /// 归一跟踪窗口,秒:响度的近期峰值 / 谷值按此窗口收敛,跳动幅度 = 当前响度在
    /// 两者之间的位置;越大对段落起伏越敏感,越小越趋向恒定满幅。
    gain_window_secs: f32,

    /// 低通滤波截止频率,Hz:响度测量只计低频能量,包络只跟底鼓 / 贝斯走,人声
    /// 与镲片不触发跳动;0 = 不滤波(全频段)。
    bass_cutoff_hz: f32,

    /// gamma 感知曲线:归一响度的幂次。大于 1 压低中低响度、鼓点间隙沉得更干净;
    /// 1 = 线性。
    gamma: f32,

    /// punch 瞬态通道(专抓鼓点的「点」)。
    punch: PunchConfig,
}

/// 氛围响度跳动的调制深度表(挂在 `PulseConfig` 下):响度包络(0-1)乘各深度后
/// 作用到对应旋钮;0 = 该目标不随响度变化,多目标可叠加。
#[config_section]
pub struct PulseDepthConfig {
    /// 场浓度增量 0-1:满响度时在 `intensity` 上叠加的量——最直觉的「颜色跳动」;
    /// 过大明暗闪烁刺眼。
    intensity: f32,

    /// 色斑半径膨胀比例 0-1:满响度时 `sigma` 放大到 `1 + 此值` 倍,色斑随鼓点
    /// 呼吸;单独开时颜色深浅不变。
    sigma: f32,

    /// 亮端推 0-1:满响度时各锚点的色带采样位向亮端推进的比例(1 = 暗端一推到底)
    /// ——颜色本身变亮而非变浓。
    brightness: f32,

    /// 暗角减弱比例 0-1:满响度时 `vignette.strength` 打的折扣,响时场向边缘涌;
    /// 易显「泵感」,慎用。
    vignette: f32,
}

/// 氛围响度跳动的瞬态通道(挂在 `PulseConfig` 下):零 attack、快 release 的第二条
/// 包络,与主包络取较大者合成——主包络管持续的「呼吸」,瞬态通道管鼓点瞬间的「跳」。
#[config_section]
pub struct PunchConfig {
    /// 混入强度 0-1:瞬态包络乘此系数后与主包络取较大者;0 = 关闭,只留主包络。
    gain: f32,

    /// 瞬态的释放时长,毫秒:越小「点」越锐利,越大越接近主包络的平滑。
    release_ms: u32,
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

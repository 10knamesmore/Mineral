//! 响度包络段(挂在 `AudioConfig` 下):波形 seekbar 的离线包络计算参数。
//!
//! 管线粒度(点数 / 块 / 滑窗)与 K-weighting 滤波参数(BS.1770 模拟原型)全部
//! 在此外置;默认值即规范参数,一般不需要动。参数变更只影响之后计算的包络,
//! 已落库的不会自动重算(只有算法版本 bump 才触发重算)。

use mineral_config_macros::config_section;

/// 响度包络计算配置。
#[config_section]
pub struct EnvelopeConfig {
    /// 包络定长点数(产出粒度,渲染端再按显示宽度二次重采样)。
    points: usize,

    /// 响度块时长(毫秒):块内取均方,块粒度按采样率折算成帧数。
    block_ms: u32,

    /// momentary 滑窗时长(毫秒):对齐 BS.1770 momentary 为 4 × 块时长(75% 重叠)。
    window_ms: u32,

    /// K-weighting 第一级:高频搁架(头部声学)滤波参数。
    shelf: ShelfConfig,

    /// K-weighting 第二级:RLB 高通(人耳低频不敏感)滤波参数。
    highpass: HighpassConfig,
}

/// 高频搁架滤波参数(模拟原型,按采样率经双线性变换推导数字系数)。
#[config_section]
pub struct ShelfConfig {
    /// 转折频率(Hz)。
    f0_hz: f64,

    /// 搁架增益(dB)。
    gain_db: f64,

    /// 品质因数。
    q: f64,

    /// 过渡带增益分配指数(`Vb = Vh^band_exponent`);与其余参数同为参考实现
    /// 从规范系数表反推的模拟原型参数。
    band_exponent: f64,
}

/// RLB 高通滤波参数(模拟原型,按采样率经双线性变换推导数字系数)。
#[config_section]
pub struct HighpassConfig {
    /// 转折频率(Hz)。
    f0_hz: f64,

    /// 品质因数。
    q: f64,
}

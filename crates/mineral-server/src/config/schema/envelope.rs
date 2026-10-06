//! 响度包络段(挂在 `AudioConfig` 下):波形 seekbar 的离线包络计算参数。
//!
//! 管线粒度(点数 / 块 / 滑窗)与 K-weighting 滤波参数(BS.1770 模拟原型)全部
//! 在此外置;默认值即规范参数,一般不需要动。参数变更只影响之后计算的包络,
//! 已落库的不会自动重算(只有算法版本 bump 才触发重算)。

use mineral_config_macros::config_section;

/// 响度包络计算，修改后不重算已有包络
#[config_section]
pub struct EnvelopeConfig {
    /// 包络采样点数
    points: usize,

    /// 响度分块时长
    block_ms: u32,

    /// 响度滑窗时长
    window_ms: u32,

    /// 高频搁架滤波
    shelf: ShelfConfig,

    /// 低频高通滤波
    highpass: HighpassConfig,
}

/// 高频搁架滤波
#[config_section]
pub struct ShelfConfig {
    /// 转折频率
    f0_hz: f64,

    /// 搁架增益
    gain_db: f64,

    /// 品质因数
    q: f64,

    /// 过渡带增益分配指数
    band_exponent: f64,
}

/// RLB 高通滤波
#[config_section]
pub struct HighpassConfig {
    /// 转折频率
    f0_hz: f64,

    /// 品质因数
    q: f64,
}

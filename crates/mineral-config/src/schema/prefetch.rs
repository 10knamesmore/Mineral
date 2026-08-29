//! 预取段(挂在 `TuiConfig` 下):各 lookahead 半径 + 去抖。

use mineral_config_macros::config_section;

/// 预取配置。
#[config_section]
pub struct PrefetchConfig {
    /// 通用预取半径:浏览列表选中行上下各预取此数条。封面压缩源落磁盘并生成当前布局的
    /// 低清 preview，完整解码图不因半径常驻 RAM；同时预取歌单曲目。
    radius: usize,

    /// 在播曲封面预取半径:沿播放队列前后各预取此数张,服务自动切歌。
    playback_cover_radius: usize,

    /// 全屏预热沿播放队列向前看的首数:提前编码封面,消自动切歌瞬间的空白闪;
    /// 每首一份终端图开销。
    prewarm_ahead: usize,
}

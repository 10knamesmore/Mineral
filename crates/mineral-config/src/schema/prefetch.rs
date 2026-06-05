//! 预取段(挂在 `TuiConfig` 下):各 lookahead 半径 + 去抖。

use serde::Deserialize;

/// 预取配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PrefetchConfig {
    /// 通用预取半径(覆盖视口 + 跳跃 lookahead)。
    radius: usize,

    /// 在播曲封面预取半径(沿播放队列)。
    playback_cover_radius: usize,

    /// 播放计数查询去抖(毫秒)。
    play_count_debounce_ms: u64,

    /// 全屏预热沿播放队列向前看的首数。
    prewarm_ahead: usize,
}

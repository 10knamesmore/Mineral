//! 普通进度条与波形共用的轨道层级和播放头着色。

use mineral_config_macros::config_section;

/// 播放进度条配色；两种形态逐帧读取同一组参数。
#[config_section]
pub struct ProgressConfig {
    /// 轨道相对当前背景的明暗层级
    track: ProgressTrackConfig,

    /// 播放头附近的强调色范围
    playhead: PlayheadConfig,
}

/// 轨道颜色由当前位置背景与主题 text 按比例混合。
#[config_section]
pub struct ProgressTrackConfig {
    /// 已播放段的文本混色比例，0-1
    played_alpha: f32,

    /// 未播放但已缓冲段的文本混色比例，0-1
    buffered_alpha: f32,

    /// 未缓冲段的文本混色比例，0-1
    unbuffered_alpha: f32,
}

/// 播放头使用主题 accent；动态主题开启时随封面换色。
#[config_section]
pub struct PlayheadConfig {
    /// 向主题 text 混色的比例，0-1；提高可淡化强调色
    text_mix: f32,

    /// 播放头后方向已播色淡出的距离，字符列
    trail_columns: u16,

    /// 播放头前方向轨道色淡出的距离，字符列
    lead_columns: u16,
}

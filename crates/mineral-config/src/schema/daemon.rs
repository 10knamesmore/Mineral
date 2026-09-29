//! daemon 段:gapless 预取 + 播放器/服务端各间隔节拍。
//!
//! 这些是领域/后端逻辑(非 TUI 交互手感):prev 分界、循环节拍、心跳、上报间隔等。

use mineral_config_macros::config_section;

/// 后台服务
#[config_section]
pub struct DaemonConfig {
    /// 曲尾预取下一首的提前量
    gapless_prefetch_ms: u64,

    /// 上一首操作分界；超过则回当前曲开头
    prev_restart_threshold_ms: u64,

    /// 播放器轮询间隔
    player_tick_ms: u64,

    /// 播放进度保存间隔
    session_save_secs: u64,

    /// 状态心跳日志间隔
    heartbeat_secs: u64,

    /// 系统媒体控件进度上报间隔
    report_interval_ms: u64,

    /// 跳转识别阈值，应小于最小跳转步长
    seek_threshold_ms: u64,

    /// 下载测速刷新周期
    download_speed_tick_ms: u64,

    /// 各 channel 任务并发数，至少 1
    channel_workers_per: usize,
}

//! daemon 段:gapless 预取 + 播放器/服务端各间隔节拍。
//!
//! 这些是领域/后端逻辑(非 TUI 交互手感):prev 分界、循环节拍、心跳、上报间隔等。

use mineral_config_macros::config_section;

/// daemon 段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct DaemonConfig {
    /// gapless 预取提前量(毫秒):距当前曲结束多久开始预取下一首。
    gapless_prefetch_ms: u64,

    /// `prev` 键「回开头 vs 上一首」分界(毫秒)。
    prev_restart_threshold_ms: u64,

    /// player 主循环醒来间隔(毫秒)。
    player_tick_ms: u64,

    /// 会话「位置刷新」节流间隔(秒)。
    session_save_secs: u64,

    /// client 心跳间隔(秒)。
    heartbeat_secs: u64,

    /// 播放进度上报间隔(毫秒)。
    report_interval_ms: u64,

    /// 判定为 seek 的位置跳变阈值(毫秒)。
    seek_threshold_ms: u64,

    /// 下载测速刷新周期(毫秒)。
    download_speed_tick_ms: u64,

    /// 每个 channel 的任务 worker 数(user/bg 两级队列共享)。
    channel_workers_per: usize,
}

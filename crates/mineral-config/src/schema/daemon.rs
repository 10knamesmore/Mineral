//! daemon 段:gapless 预取 + 播放器/服务端各间隔节拍。
//!
//! 这些是领域/后端逻辑(非 TUI 交互手感):prev 分界、循环节拍、心跳、上报间隔等。

use mineral_config_macros::config_section;

/// daemon 段。
#[config_section]
pub struct DaemonConfig {
    /// gapless 预取提前量(毫秒):距当前曲结束多久开始预取下一首;太小可能退化出间隙。
    gapless_prefetch_ms: u64,

    /// `prev` 键「回开头 vs 上一首」分界(毫秒):播放进度超过此值按回开头,否则上一首。
    prev_restart_threshold_ms: u64,

    /// player 主循环醒来间隔(毫秒);影响自动切歌 / 事件转发延迟。
    player_tick_ms: u64,

    /// 会话「位置刷新」节流间隔(秒):播放进度周期落盘;切歌等状态变化另有即时落盘。
    session_save_secs: u64,

    /// 状态心跳日志间隔(秒);daemon 与 client 各打一条,供事后排查。
    heartbeat_secs: u64,

    /// 向系统媒体控件(MPRIS)上报播放进度的间隔(毫秒);影响桌面歌词 / 控件同步平滑度。
    report_interval_ms: u64,

    /// 判定为 seek 的位置跳变阈值(毫秒):进度偏离线性预期超过此值按 seek 上报给媒体控件;
    /// 需远大于节拍抖动、远小于最小 seek 步长。
    seek_threshold_ms: u64,

    /// 下载测速刷新周期(毫秒)。
    download_speed_tick_ms: u64,

    /// 每个 channel 的任务 worker 数(user/bg 两级队列共享),≥1;大了抓取快但更容易撞源限流。
    channel_workers_per: usize,
}

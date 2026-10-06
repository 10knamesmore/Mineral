//! daemon 配置根;只接受音频、音乐来源和服务运行字段。

use mineral_config_macros::config_section;

use super::audio::AudioConfig;
use super::cache::CacheConfig;
use super::download::DownloadConfig;
use super::queue::QueueConfig;
use super::script::ScriptConfig;
use super::sources::SourcesConfig;
use super::stats::StatsConfig;

/// daemon 的音频、音乐来源与服务运行配置
#[config_section]
#[lua_extra_field(
    "setup?",
    "fun(api: mineral.DaemonApi): nil",
    "daemon 启动或成功重载时执行;api 提供音乐、日志和 daemon 配置操作"
)]
pub struct DaemonConfig {
    /// 音频播放
    audio: AudioConfig,

    /// 音频缓存
    cache: CacheConfig,

    /// 下载
    download: DownloadConfig,

    /// 音乐来源
    sources: SourcesConfig,

    /// 播放队列
    queue: QueueConfig,

    /// 曲尾预取下一首的提前量，毫秒
    gapless_prefetch_ms: u64,

    /// 上一首操作分界，毫秒；进度超过此值则回当前曲开头
    prev_restart_threshold_ms: u64,

    /// 播放器轮询间隔，毫秒
    player_tick_ms: u64,

    /// 播放进度保存间隔，秒
    session_save_secs: u64,

    /// daemon 状态心跳日志间隔，秒
    heartbeat_secs: u64,

    /// 系统媒体控件进度上报间隔，毫秒
    report_interval_ms: u64,

    /// 跳转识别阈值，毫秒；应小于最小跳转步长
    seek_threshold_ms: u64,

    /// 下载测速刷新周期，毫秒
    download_speed_tick_ms: u64,

    /// 各 channel 任务并发数，至少 1
    channel_workers_per: usize,

    /// 脚本运行
    script: ScriptConfig,

    /// 行为统计
    stats: StatsConfig,
}

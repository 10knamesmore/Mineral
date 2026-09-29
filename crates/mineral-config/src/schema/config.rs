//! 顶层 [`Config`] 聚合各域,以及 TUI client 命名空间 [`TuiConfig`]。
//!
//! `default.lua` 与用户 `config.lua` 深合并后整表一次反序列化落成 [`Config`];
//! 各域子段经各自 getter 读取。

use mineral_config_macros::config_section;

use super::ambient::AmbientConfig;
use super::animation::AnimationConfig;
use super::audio::AudioConfig;
use super::behavior::BehaviorConfig;
use super::cache::CacheConfig;
use super::copy::CopyConfig;
use super::cover::{CoverConfig, CoverTransitionConfig};
use super::daemon::DaemonConfig;
use super::download::DownloadConfig;
use super::keys::KeysConfig;
use super::layout::LayoutConfig;
use super::lyrics::LyricsConfig;
use super::minimap::MinimapConfig;
use super::prefetch::PrefetchConfig;
use super::queue::QueueConfig;
use super::script::ScriptConfig;
use super::search::SearchConfig;
use super::sources::SourcesConfig;
use super::spectrum::SpectrumConfig;
use super::stats::StatsConfig;
use super::theme::ThemeConfig;
use super::toast::ToastConfig;
use super::waveform::WaveformConfig;
use super::window_title::WindowTitleConfig;

/// Mineral 配置
#[config_section]
pub struct Config {
    /// 终端界面
    tui: TuiConfig,

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

    /// 后台服务
    daemon: DaemonConfig,

    /// 脚本运行
    script: ScriptConfig,

    /// 行为统计
    stats: StatsConfig,
}

/// 终端界面配置
#[config_section]
pub struct TuiConfig {
    /// 主题配色
    theme: ThemeConfig,

    /// 按键绑定
    keys: KeysConfig,

    /// 交互行为
    behavior: BehaviorConfig,

    /// 频谱面板
    spectrum: SpectrumConfig,

    /// 进度条波形
    waveform: WaveformConfig,

    /// 封面加载与缓存
    cover: CoverConfig,

    /// 全屏封面转场
    cover_transition: CoverTransitionConfig,

    /// 全屏氛围背景
    ambient: AmbientConfig,

    /// 列表与封面预取
    prefetch: PrefetchConfig,

    /// 搜索
    search: SearchConfig,

    /// 歌词
    lyrics: LyricsConfig,

    /// 动画
    animation: AnimationConfig,

    /// 列表缩略图
    minimap: MinimapConfig,

    /// 通知
    toast: ToastConfig,

    /// 布局
    layout: LayoutConfig,

    /// 复制模板
    copy: CopyConfig,

    /// 窗口标题
    window_title: WindowTitleConfig,
}

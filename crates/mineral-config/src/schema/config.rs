//! 顶层 [`Config`] 聚合各域,以及 TUI client 命名空间 [`TuiConfig`]。
//!
//! `default.lua` 与用户 `config.lua` 深合并后整表一次反序列化落成 [`Config`];
//! 各域子段经各自 getter 读取。

use mineral_config_macros::config_section;

use super::animation::AnimationConfig;
use super::audio::AudioConfig;
use super::behavior::BehaviorConfig;
use super::cache::CacheConfig;
use super::copy::CopyConfig;
use super::cover::CoverConfig;
use super::daemon::DaemonConfig;
use super::download::DownloadConfig;
use super::keys::KeysConfig;
use super::layout::LayoutConfig;
use super::lyrics::LyricsConfig;
use super::prefetch::PrefetchConfig;
use super::script::ScriptConfig;
use super::search::SearchConfig;
use super::sources::SourcesConfig;
use super::spectrum::SpectrumConfig;
use super::theme::ThemeConfig;
use super::toast::ToastConfig;
use super::waveform::WaveformConfig;
use super::window_title::WindowTitleConfig;

/// 用户运行期配置的强类型真相源。深合并后整表一次反序列化落成本类型。
///
/// 字段私有 + `#[non_exhaustive]`:外部只能经 [`crate::load`] 或 [`crate::Config::defaults`]
/// 取得,经 getter 读取,不可字面量构造(对外配置 struct 约定)。
#[config_section]
pub struct Config {
    /// TUI client 段:in-repo client 专属命名空间,内含 theme + keys + behavior。
    tui: TuiConfig,

    /// 音频段(音量 / 后端)。
    audio: AudioConfig,

    /// 缓存容量段。
    cache: CacheConfig,

    /// 下载段。
    download: DownloadConfig,

    /// 音乐源段。
    sources: SourcesConfig,

    /// daemon 段。
    daemon: DaemonConfig,

    /// 脚本运行时段(watchdog 双阈值)。
    script: ScriptConfig,
}

/// TUI client 配置命名空间。把主题 / 键位 / 交互手感收进 client 段:TUI 是
/// in-repo client,在协议上无特权,只有「打包特权」。第三方 client 的配置活在
/// 自己生态,不进本文件;未来 in-repo client 平行加段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取:`cfg.tui().theme()` 等。
#[config_section]
pub struct TuiConfig {
    /// 主题色板段(14 token + 3 roles)。
    theme: ThemeConfig,

    /// 键位重映射段(动作 → 键)。
    keys: KeysConfig,

    /// 交互手感段(音量/seek 步长、列表跳行、daemon 续命开关)。
    behavior: BehaviorConfig,

    /// 频谱面板段(观感开关 + 平滑/衰减 + peak 物理)。
    spectrum: SpectrumConfig,

    /// 进度条波形段(振幅波形开关 + 封面取色开关)。
    waveform: WaveformConfig,

    /// 封面段(抓取/缓存/并发 + kmeans 取色)。
    cover: CoverConfig,

    /// 预取段(各 lookahead 半径 + 去抖 + 抓取并发)。
    prefetch: PrefetchConfig,

    /// 搜索段:`deep`(本地过滤搜索行为旋钮)与 `channel`(远程搜索白名单)两个子段。
    search: SearchConfig,

    /// 歌词段(行距 + 滚动手感)。
    lyrics: LyricsConfig,

    /// 动画段(帧率 + 各转场/扫入时长 + 视图扫入风格)。
    animation: AnimationConfig,

    /// toast 段(通知停留时长)。
    toast: ToastConfig,

    /// 布局段(完整布局门槛 + 全屏分区尺寸 + 浮层 dock 宽)。
    layout: LayoutConfig,

    /// copy 段(复制菜单的自定义模板)。
    copy: CopyConfig,

    /// 窗口标题段(终端任务栏 / tab 标题)。
    window_title: WindowTitleConfig,
}

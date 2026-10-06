//! 当前 TUI 的本地配置根;不接受 daemon 配置字段。

use std::collections::BTreeMap;

use mineral_config_macros::config_section;

use super::ambient::AmbientConfig;
use super::animation::AnimationConfig;
use super::behavior::BehaviorConfig;
use super::copy::CopyConfig;
use super::cover::{CoverConfig, CoverTransitionConfig};
use super::keys::KeysConfig;
use super::layout::LayoutConfig;
use super::lyrics::LyricsConfig;
use super::minimap::MinimapConfig;
use super::prefetch::PrefetchConfig;
use super::progress::ProgressConfig;
use super::script::TuiScriptConfig;
use super::search::SearchConfig;
use super::spectrum::SpectrumConfig;
use super::theme::{ColorRef, ThemeConfig};
use super::toast::ToastConfig;
use super::waveform::WaveformConfig;
use super::window_title::WindowTitleConfig;

/// TUI 的本地配置，不从 daemon 接收配置树
#[config_section]
#[lua_extra_field(
    "setup?",
    "fun(api: mineral.TuiApi): nil",
    "TUI 启动或成功重载时执行;api 只影响当前 TUI"
)]
pub struct TuiConfig {
    /// 各来源的徽标颜色，键为来源名
    source_colors: BTreeMap<String, ColorRef>,

    /// 当前 TUI 状态心跳日志间隔，秒
    heartbeat_secs: u64,

    /// 本地 Lua 执行预算
    script: TuiScriptConfig,

    /// 主题配色
    theme: ThemeConfig,

    /// 按键绑定
    keys: KeysConfig,

    /// 交互行为
    behavior: BehaviorConfig,

    /// 频谱面板
    spectrum: SpectrumConfig,

    /// 播放进度条配色
    progress: ProgressConfig,

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

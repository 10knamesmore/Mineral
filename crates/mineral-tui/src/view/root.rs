//! 保留界面组件实例的树；领域数据与资源服务通过每次调用借入。

use crate::components::layout::browse::spectrum::SpectrumState;
use crate::components::layout::shared::transport::TransportBar;
use crate::render::anim::{Toggle, ticks16_from_ms};
use crate::runtime::state::{BrowsePage, SearchPage, search_whitelist};
use ratatui::layout::Rect;

/// 根界面只拥有组件与页面显示状态。
pub(crate) struct RootView {
    /// Browse 布局层的 view 状态:视图切换 + 全屏子模式 + 列表导航 + 歌词 + `/` 过滤,
    /// 由 [`BrowsePage`] 聚合。与 model 数据(library / player)分离。
    pub(crate) browse: BrowsePage,

    /// channel 搜索布局态:与 [`BrowsePage::fullscreen`] 同级的全屏级布局态(两者逻辑 `on` 互斥)。
    /// 含布局开关 + 当前源 + 输入焦点 + 焦点环 + per-源会话。
    pub(crate) channel_search: SearchPage,

    /// 顶栏失焦变灰:`on()` = 已变灰(终端未聚焦)、`eased_in_out()` = 变灰深度。
    /// 初始 `off`(聚焦)——mode 1004 只报变化、
    /// 不支持的终端永不发事件,降级方向必须是「恒聚焦」。
    pub(crate) dim: Toggle,

    /// 播放栏本地反馈；动作唤起，显式 tick 推进，绘制只读。
    pub(crate) transport: TransportBar,

    /// 频谱状态(条高 + 平滑)。
    pub(crate) spectrum: SpectrumState,

    /// 最近一次准备的主帧面积；输入路径据此计算弹出菜单锚点。
    pub(crate) frame_area: Rect,

    /// 主循环采样的本地钟点，供预计播完时间等显示使用。
    pub(crate) now: chrono::DateTime<chrono::Local>,

    /// 本次准备采样的单调时钟；绘制不查询系统时间。
    pub(crate) frame_now: std::time::Instant,

    /// not playing 待机唱片纹的旋转状态(相位每 tick 推进,节奏来自配置
    /// `animation.vinyl_rev_ms`)。
    pub(crate) vinyl: crate::components::layout::shared::vinyl::VinylSpin,
}

impl RootView {
    /// 按当前配置创建组件实例，后续配置通过应用更新入口重设折算参数。
    pub(crate) fn new(cfg: &mineral_config::Config, mode: mineral_protocol::PlayMode) -> Self {
        let anim = cfg.tui().animation();
        let tick_ms = *anim.frame_tick_ms();
        Self {
            browse: BrowsePage::new(anim),
            channel_search: SearchPage::new(
                ticks16_from_ms(*anim.fullscreen_ms(), tick_ms),
                ticks16_from_ms(*anim.search_focus_morph_ms(), tick_ms),
            )
            .with_whitelist(search_whitelist::SearchWhitelist::from(
                cfg.tui().search().channel(),
            )),
            dim: Toggle::new(ticks16_from_ms(*anim.focus_fade_ms(), tick_ms)),
            transport: TransportBar::new(mode, anim),
            spectrum: SpectrumState::new(cfg.tui().spectrum().clone(), tick_ms),
            vinyl: crate::components::layout::shared::vinyl::VinylSpin::from_config(
                *anim.vinyl_rev_ms(),
                tick_ms,
            ),
            frame_area: Rect::default(),
            now: chrono::Local::now(),
            frame_now: std::time::Instant::now(),
        }
    }
}

//! 当前页面、布局层与输入焦点的判定，以及封面滚动防抖状态。

use std::time::Duration;

use super::{AppState, SearchFocus};

/// 左栏当前展示的视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// 歌单列表(默认)。
    Playlists,

    /// 已选歌单内的曲目列表。
    Library,
}

/// 当前活跃的布局层(浮层栈之下)。`handle_key` 路由与 `collect_key_context` 共用
/// [`AppState::active_layer`] 算它——「在哪层」只一处真相,杜绝两处漂移。浮层栈叠在其上,
/// 由调用方各自裁决(路由对所有浮层一视同仁;脚本 ctx 只认 queue 浮层光标)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveLayer {
    /// channel 搜索布局态。
    SearchSession,

    /// 本地 `/` 模糊搜索输入态。
    DeepSearch,

    /// 全屏播放态。
    Fullscreen,

    /// 默认浏览态。
    Browse,
}

/// 当前激活的页(浮层栈之下),供按键路由分流到对应 [`Page`](crate::app) 实现。
///
/// 只两页:`Search` 是模态(独立输入树),其余一律 `Browse`——fullscreen / `/` 过滤都是
/// Browse 同一套导航面上的子模式,不另起页。比 [`ActiveLayer`] 粗一档(后者细分子模式供
/// 渲染 / 上下文裁决)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    /// channel 搜索模态页。
    Search,

    /// 浏览页(含 fullscreen / deep-search 子模式)。
    Browse,
}

impl AppState {
    /// 把应用动画与滚动状态折算为图片引擎需要的互斥渲染阶段。
    pub(crate) fn image_render_phase(&self) -> crate::image::ImageRenderPhase {
        if !self.browse.fullscreen.settled()
            || !self.channel_search.active.settled()
            || !(self.browse.view.at_min() || self.browse.view.at_max())
        {
            crate::image::ImageRenderPhase::Resizing
        } else if self.is_scrolling() {
            crate::image::ImageRenderPhase::Scrolling
        } else {
            crate::image::ImageRenderPhase::Stable
        }
    }

    /// 终端是否持有输入焦点。从 [`Self::dim`] 反读(变灰 = 未聚焦);上报 daemon 用。
    pub fn focused(&self) -> bool {
        !self.dim.on()
    }

    /// 是否处于文本输入态:本地 `/` 模糊 typing,或 channel-search 搜索框(prompt 焦点)。
    /// 全局逃生口 / 单键快捷在此让位字符输入——输入态的按键是文本,不是命令。
    pub(crate) fn in_text_input(&self) -> bool {
        self.browse.search.typing
            || (self.channel_search.active.on() && self.channel_search.focus == SearchFocus::Prompt)
    }

    /// 当前激活的页(见 [`PageKind`]):Search 模态优先,否则归 Browse。供 `handle_key` 顶层
    /// 路由分流到对应 Page 实现;子模式细分仍读 [`Self::active_layer`]。
    pub(crate) fn page_kind(&self) -> PageKind {
        if self.channel_search.active.on() {
            PageKind::Search
        } else {
            PageKind::Browse
        }
    }

    /// 当前活跃的布局层(见 [`ActiveLayer`])。浮层栈在其之上,由调用方单独裁决。
    pub(crate) fn active_layer(&self) -> ActiveLayer {
        if self.channel_search.active.on() {
            ActiveLayer::SearchSession
        } else if self.browse.search.typing {
            ActiveLayer::DeepSearch
        } else if self.browse.fullscreen.on() {
            ActiveLayer::Fullscreen
        } else {
            ActiveLayer::Browse
        }
    }

    /// 距上次选中变化是否仍在封面 debounce 防抖窗口内(配置 `tui.cover.debounce_ms`)。
    ///
    /// 时间戳按活跃 surface 取:search 布局态看搜索面板的选中变化,否则看 browse 列表的
    /// ——两边各自维护,防抖只对当前正在滚的那个面板生效。
    pub fn is_scrolling(&self) -> bool {
        let last_sel_change = if self.channel_search.active.on() {
            self.channel_search.last_sel_change
        } else {
            self.browse.nav.last_sel_change
        };
        last_sel_change.elapsed() < Duration::from_millis(*self.cfg.tui().cover().debounce_ms())
    }
}

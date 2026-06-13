//! channel 搜索布局态域([`AppState`](crate::runtime::state::AppState) 顶层,与全屏态同级):
//! Search 布局态开关 + 当前源 + 输入焦点 + 焦点环形变 + per-源会话。
//!
//! 与本地模糊过滤([`SearchState`](crate::runtime::state::SearchState))是两回事:那边是「单段
//! 文本怎么匹配」,这边是一个全屏级布局态(进入后整屏切到搜索视图)。

use mineral_channel_core::ChannelCaps;
use mineral_model::{SearchKind, SourceKind};
use mineral_task::SearchPayload;
use rustc_hash::FxHashMap;

use crate::render::anim::{Toggle, Transition};

/// Search 布局态的输入焦点:token prompt 打字 / 结果列导航 / 详情面板。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchFocus {
    /// token prompt 输入态
    Prompt,

    /// 结果列导航态
    Results,

    /// 详情面板态(实体详情 / 后续操作)
    Detail,
}

impl SearchFocus {
    /// 是否为结果/详情面板(非 prompt 输入态)——切到面板时据此记 `last_panel`。
    fn is_panel(self) -> bool {
        matches!(self, Self::Results | Self::Detail)
    }
}

/// 单个源的channel 搜索会话:实体类型 + 输入词 + 最近一页结果 + 光标。
///
/// `@` 切源切的是整个会话(各源独立、切回恢复);本结构是其载体。
pub struct ChannelSearch {
    /// 搜索实体类型(取自源 `caps.searchable`;`$` 类型菜单切换待接入)。
    pub kind: SearchKind,

    /// token prompt 当前输入词(Enter 时提交、回包按它配对)。
    pub query: String,

    /// 最近一页结果(`None` = 尚未搜索 / loading 占位)。
    pub results: Option<SearchPayload>,

    /// 结果列光标下标。
    pub sel: usize,
}

impl ChannelSearch {
    /// 当前结果条数(尚未搜索 / 无结果计 0)。
    pub fn result_len(&self) -> usize {
        match &self.results {
            None => 0,
            Some(SearchPayload::Songs(items)) => items.len(),
            Some(SearchPayload::Albums(items)) => items.len(),
            Some(SearchPayload::Playlists(items)) => items.len(),
            Some(SearchPayload::Artists(items)) => items.len(),
        }
    }
}

/// channel 搜索布局态:Search 布局态开关 + 当前源 + 输入焦点 + 焦点环 + per-channel 会话。
///
/// `active` 与全屏态同级语义(全局布局态),两者逻辑 `on` 同时只一个(互斥)。
pub struct ChannelSearchState {
    /// Search 布局态开关:`on()` 供按键路由互斥,`eased_in_out()` 供布局 morph 渲染位置。
    pub active: Toggle,

    /// 当前搜索源(进入时落到首个 searchable 源;无可搜索源为 `None` = 空态)。
    pub source: Option<SourceKind>,

    /// 输入焦点(prompt 打字 / results 导航 / detail 详情)。
    pub focus: SearchFocus,

    /// 焦点环滑动的起点焦点:border morph 期间从此面板矩形滑向 `focus` 矩形。
    pub prev_focus: SearchFocus,

    /// 焦点环滑动进度(`prev_focus` → `focus`)。settled 时渲染落到 `focus` 矩形;
    /// 飞行中按 `eased_in_out` 在两矩形间 lerp(border_morph 开时)。
    pub focus_ring: Transition,

    /// 最近停留的面板焦点(Results/Detail):prompt 态「只切不搜」时据此回原位。
    pub last_panel: SearchFocus,

    /// 焦点环滑动拍数(独立于布局进退场,源自 `animation.search_focus_morph_ms`),`set_focus`
    /// 据此重新 arm 滑动。
    ring_ticks: u16,

    /// per-源搜索会话(query/kind/结果/光标独立,切源恢复)。
    pub sessions: FxHashMap<SourceKind, ChannelSearch>,
}

impl ChannelSearchState {
    /// 构造收起态的channel 搜索域。
    ///
    /// # Params:
    ///   - `layout_ticks`: 布局态进退场 morph 拍数(复用全屏节拍)
    ///   - `ring_ticks`: 焦点环滑动拍数(独立旋钮,见 [`Self::ring_ticks`])
    pub(crate) fn new(layout_ticks: u16, ring_ticks: u16) -> Self {
        Self {
            active: Toggle::new(layout_ticks),
            source: None,
            focus: SearchFocus::Prompt,
            prev_focus: SearchFocus::Prompt,
            focus_ring: Transition::new(ring_ticks),
            last_panel: SearchFocus::Results,
            ring_ticks,
            sessions: FxHashMap::default(),
        }
    }

    /// 切换输入焦点并驱动焦点环滑动(border morph)。同焦点为 no-op(不重置已 settle
    /// 的环)。切到面板(Results/Detail)时记住为 `last_panel`,供 prompt 态「只切不搜」回位。
    ///
    /// # Params:
    ///   - `focus`: 目标焦点
    pub(crate) fn set_focus(&mut self, focus: SearchFocus) {
        if focus == self.focus {
            return;
        }
        self.prev_focus = self.focus;
        self.focus = focus;
        self.focus_ring = Transition::expanding(self.ring_ticks);
        if focus.is_panel() {
            self.last_panel = focus;
        }
    }

    /// 推进布局形变与焦点环各一拍(同帧驱动,由主循环 tick 调用)。
    pub(crate) fn tick(&mut self) {
        self.active.tick();
        self.focus_ring.tick();
    }

    /// 进入 Search 布局态:挑默认搜索源 + 确保其会话存在,焦点回 prompt。
    ///
    /// 默认源 = 首个 `searchable` 非空的源(按 `name()` 定序去抖,多源下确定);
    /// 默认 kind = 该源 `searchable` 首项。已有源则保留(切回恢复),仅复位焦点。
    /// 无可搜索源时 `source` 留 `None`(空态由渲染层提示)。
    ///
    /// # Params:
    ///   - `caps`: 各源能力声明(决定哪些源可搜、默认类型)
    pub(crate) fn enter(&mut self, caps: &FxHashMap<SourceKind, ChannelCaps>) {
        // 焦点落 prompt 且焦点环复位:布局形变本身已是进场动画,无需叠加面板内滑动。
        self.focus = SearchFocus::Prompt;
        self.prev_focus = SearchFocus::Prompt;
        self.focus_ring = Transition::new(self.ring_ticks);
        if self.source.is_some() {
            return;
        }
        // 首个 searchable 源,按 name() 定序去抖(FxHashMap 迭代序不确定)。
        let mut candidates: Vec<(&SourceKind, &ChannelCaps)> = caps
            .iter()
            .filter(|(_, caps)| !caps.searchable().is_empty())
            .collect();
        candidates.sort_by_key(|(src, _)| src.name());
        let Some((src, caps)) = candidates.first() else {
            return;
        };
        let src = **src;
        let kind = caps
            .searchable()
            .first()
            .copied()
            .unwrap_or(SearchKind::Song);
        self.source = Some(src);
        self.sessions.entry(src).or_insert(ChannelSearch {
            kind,
            query: String::new(),
            results: None,
            sel: 0,
        });
    }

    /// 当前源的搜索会话(只读)。
    pub fn current(&self) -> Option<&ChannelSearch> {
        self.source.and_then(|s| self.sessions.get(&s))
    }

    /// 当前源的搜索会话(可变;打字 / 翻页写入用)。
    pub fn current_mut(&mut self) -> Option<&mut ChannelSearch> {
        match self.source {
            Some(src) => self.sessions.get_mut(&src),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::ChannelCaps;
    use mineral_model::{SearchKind, SourceKind};
    use rustc_hash::FxHashMap;

    use super::{ChannelSearchState, SearchFocus};

    /// 进入时挑首个 searchable 源、kind 落到该源 searchable 首项、焦点回 prompt。
    #[test]
    fn enter_picks_first_searchable_source() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        let mut caps = FxHashMap::<SourceKind, ChannelCaps>::default();
        caps.insert(
            SourceKind::NETEASE,
            ChannelCaps::builder()
                .searchable(vec![SearchKind::Album, SearchKind::Song])
                .playlist_edit(false)
                .build(),
        );
        let mut rs = ChannelSearchState::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1);
        rs.enter(&caps);

        assert_eq!(
            rs.source,
            Some(SourceKind::NETEASE),
            "落到唯一 searchable 源"
        );
        assert_eq!(rs.focus, SearchFocus::Prompt, "焦点回 prompt");
        let session = rs.current().ok_or_else(|| eyre!("入会后应有当前会话"))?;
        assert_eq!(session.kind, SearchKind::Album, "kind 落到 searchable 首项");
        Ok(())
    }

    /// 无 searchable 源(caps 全空)时 source 留 None(空态)。
    #[test]
    fn enter_with_no_searchable_source_stays_none() {
        let caps = FxHashMap::<SourceKind, ChannelCaps>::default();
        let mut rs = ChannelSearchState::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1);
        rs.enter(&caps);
        assert_eq!(rs.source, None, "无可搜索源 → 空态");
    }

    /// `set_focus`:切到面板记住 `last_panel`、armed 焦点环(border morph);回 prompt
    /// 不改 `last_panel`;同焦点为 no-op(不重置已 settle 的环)。
    #[test]
    fn set_focus_tracks_last_panel_and_arms_ring() {
        let mut rs = ChannelSearchState::new(/*layout_ticks*/ 1, /*ring_ticks*/ 4);
        assert_eq!(
            rs.last_panel,
            SearchFocus::Results,
            "默认 last_panel 为 results"
        );
        assert!(rs.focus_ring.settled(), "初始无滑动");

        rs.set_focus(SearchFocus::Detail);
        assert_eq!(rs.focus, SearchFocus::Detail);
        assert_eq!(
            rs.prev_focus,
            SearchFocus::Prompt,
            "环从 prompt 滑向 detail"
        );
        assert_eq!(
            rs.last_panel,
            SearchFocus::Detail,
            "停在面板 → 记住为 last_panel"
        );
        assert!(!rs.focus_ring.settled(), "切焦点 armed 滑动");

        rs.set_focus(SearchFocus::Prompt);
        assert_eq!(
            rs.last_panel,
            SearchFocus::Detail,
            "回 prompt 不改 last_panel"
        );

        // 同焦点 no-op:推满使环 settle,再 set 同焦点不重新 arm。
        for _ in 0..4 {
            rs.tick();
        }
        assert!(rs.focus_ring.settled(), "推满后环 settle");
        rs.set_focus(SearchFocus::Prompt);
        assert!(rs.focus_ring.settled(), "同焦点不重新 arm");
    }
}

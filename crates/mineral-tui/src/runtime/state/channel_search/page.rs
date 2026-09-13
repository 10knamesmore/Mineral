//! 搜索页面的布局开关、source 切换、输入焦点和下拉动画；每个 source 保留独立会话。

use std::time::Instant;

use mineral_channel_core::ChannelCaps;
use mineral_config::SearchFocusTransition;
use mineral_model::{SearchKind, SourceKind};
use rustc_hash::FxHashMap;

use crate::render::anim::{Toggle, Transition};

use super::super::search_whitelist::{self, SearchWhitelist};
use super::{KindResults, SearchSession};

/// Search 布局态的输入焦点：token prompt 打字 / 结果列导航 / 详情面板。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchFocus {
    /// token prompt 输入态
    Prompt,

    /// 结果列导航态
    Results,

    /// 实体详情与详情操作面板态
    Detail,
}

impl SearchFocus {
    /// 是否为结果/详情面板（非 prompt 输入态）——切到面板时据此记 `last_panel`。
    fn is_panel(self) -> bool {
        matches!(self, Self::Results | Self::Detail)
    }
}

/// Prompt 焦点内的段（仅 `focus == Prompt` 有意义）：source chip / kind chip / query 文本。
///
/// left/right 在三段间走——query 段内先移文本光标，到词首/词尾边界再跨到相邻 chip 段；
/// focus 落到 chip 段即自动展开其下拉（选 source / 选 kind），选定塌回高亮、Enter 重开。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptSegment {
    /// source chip 段（focus 即展开 source 下拉）。
    Source,

    /// kind chip 段（focus 即展开 kind 下拉）。
    Kind,

    /// query 文本输入段（光标编辑）。
    Query,
}

/// channel 搜索布局态：Search 布局态开关 + 当前 source + 输入焦点 + 焦点环 + per-source 会话。
///
/// `active` 与全屏态同级语义（全局布局态），两者逻辑 `on` 同时只一个（互斥）。
pub struct SearchPage {
    /// Search 布局态开关：`on()` 供按键路由互斥，`eased_in_out()` 供布局 morph 渲染位置。
    pub active: Toggle,

    /// 当前搜索 source（进入时落到首个 searchable source；无可搜索 source 为 `None` = 空态）。
    pub source: Option<SourceKind>,

    /// 输入焦点（prompt 打字 / results 导航 / detail 详情）。
    pub focus: SearchFocus,

    /// 焦点环滑动的起点焦点：border morph 期间从此面板矩形滑向 `focus` 矩形。
    pub prev_focus: SearchFocus,

    /// 焦点环滑动进度（`prev_focus` → `focus`）。settled 时渲染落到 `focus` 矩形；
    /// 飞行中按 `eased_in_out` 在两矩形间 lerp（border_morph 开时）。
    pub focus_ring: Transition,

    /// 最近停留的面板焦点（Results/Detail）：prompt 态「只切不搜」时据此回原位。
    pub last_panel: SearchFocus,

    /// 焦点环滑动拍数（独立于布局进退场，源自 `animation.search_focus_morph_ms`），`set_focus`
    /// 据此重新 arm 滑动。
    ring_ticks: u16,

    /// Prompt 焦点内的段（仅 `focus == Prompt` 有效；默认 [`PromptSegment::Query`]）。
    prompt_seg: PromptSegment,

    /// chip 段下拉是否展开（Source/Kind 段：focus 到达即开、选定塌回；Query 段恒 `false`）。
    seg_open: bool,

    /// 展开下拉里高亮行下标（按当前段对应的 source / kind 列表索引）。
    seg_sel: usize,

    /// chip 下拉的展开 / 收起动画（focus 到达重播展开、收起退场到零）。
    seg_reveal: Transition,

    /// 下拉归属的 chip 段（与 `focus` 解耦）：展开 / 收起动画期都是 `Some`，让切到 query
    /// 后仍能把上一个 chip 的收起动画画完。归零稳态由 `seg_reveal.active()` 兜停。
    reveal_seg: Option<PromptSegment>,

    /// per-source 搜索会话（query/kind/结果/光标独立，切 source 恢复）。
    sessions: FxHashMap<SourceKind, SearchSession>,

    /// source / kind 下拉的配置白名单(见 [`SearchWhitelist`])。
    whitelist: SearchWhitelist,

    /// loading spinner 帧计数（每帧 tick +1；渲染经 [`Self::spinner_counter`] 读取后选择旋转帧，
    /// 与稳态列表无关，恒推进，故 loading 占位会持续旋转）。
    spinner: u32,

    /// 最近一次「展示中封面实体」变化时刻(结果列 / 详情列光标、切区、下钻 / 返回、切桶都算)。
    /// 防抖:连续滚动时跳过封面编码与 detail 预取,稳态后再上图 / 派发——对齐 browse 列表
    /// 的同名时间戳语义。
    pub last_sel_change: Instant,
}

impl SearchPage {
    /// 构造收起态的 channel 搜索域。
    ///
    /// # Params:
    ///   - `layout_ticks`: 布局态进退场 morph 拍数（复用全屏节拍）
    ///   - `ring_ticks`: 焦点环滑动拍数（独立旋钮，见 [`Self::ring_ticks`]）
    pub(crate) fn new(layout_ticks: u16, ring_ticks: u16) -> Self {
        Self {
            active: Toggle::new(layout_ticks),
            source: None,
            focus: SearchFocus::Prompt,
            prev_focus: SearchFocus::Prompt,
            focus_ring: Transition::new(ring_ticks),
            last_panel: SearchFocus::Results,
            ring_ticks,
            prompt_seg: PromptSegment::Query,
            seg_open: false,
            seg_sel: 0,
            seg_reveal: Transition::new(ring_ticks),
            reveal_seg: None,
            sessions: FxHashMap::default(),
            whitelist: SearchWhitelist::default(),
            spinner: 0,
            last_sel_change: Instant::now(),
        }
    }

    /// 注入下拉白名单(链式,构造点接 `tui.search.channel` 配置用;不设 = 不过滤)。
    pub(crate) fn with_whitelist(mut self, whitelist: SearchWhitelist) -> Self {
        self.whitelist = whitelist;
        self
    }

    /// 配置热更:重折两档动画拍数(保留布局态 / 焦点环相位与会话)+ 换下拉白名单。
    ///
    /// # Params:
    ///   - `layout_ticks`: 布局态进退场 morph 拍数
    ///   - `ring_ticks`: 焦点环滑动拍数
    ///   - `whitelist`: 新下拉白名单
    pub(crate) fn reconfigure(
        &mut self,
        layout_ticks: u16,
        ring_ticks: u16,
        whitelist: SearchWhitelist,
    ) {
        self.active.retempo(layout_ticks);
        self.ring_ticks = ring_ticks;
        self.focus_ring.retempo(ring_ticks);
        self.seg_reveal.retempo(ring_ticks);
        self.whitelist = whitelist;
    }

    /// loading spinner 帧计数（每帧 +1）。字形选取交渲染层（按配置 `animation.spinner_frames`
    /// 取当前格，见 shared `spinner`），状态层只持帧号、不知道画什么字符。
    pub fn spinner_counter(&self) -> u32 {
        self.spinner
    }

    /// 当前会话当前 kind 是否首页搜索在飞（结果面板 loading↔empty↔idle 三态分流）。
    pub fn current_loading(&self) -> bool {
        self.current().is_some_and(|s| s.is_loading(s.kind))
    }

    /// 标记当前会话某 kind 首页搜索在飞（submit 落地时由 App 调）。
    pub(crate) fn mark_loading(&mut self, kind: SearchKind) {
        if let Some(session) = self.current_mut() {
            session.mark_in_flight(kind);
        }
    }

    /// 切换输入焦点并驱动焦点环滑动（border morph）。同焦点为 no-op（不重置已 settle
    /// 的环）。切到面板（Results/Detail）时记住为 `last_panel`，供 prompt 态「只切不搜」回位。
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
        // 回到 prompt 一律落在 query 段、下拉收起（切到面板再回来不停在半开的 chip 下拉）。
        if focus == SearchFocus::Prompt {
            self.reset_prompt_seg();
        }
    }

    /// 面板的焦点度(千分比 `0..=1000`):稳态 = 有焦点满值 / 无焦点 `0`;`Slide` 且焦点环
    /// 滑动中,旧焦点面板满值→`0`、新焦点面板 `0`→满值,与浮动环同一 eased 进度——供选中行
    /// 高亮等焦点跟随样式做颜色渐变。`Instant` 恒二态。
    ///
    /// # Params:
    ///   - `transition`: 焦点切换过渡风格(config `search_focus_transition`)
    ///   - `panel`: 询问焦点度的面板
    pub(crate) fn focus_permille(
        &self,
        transition: SearchFocusTransition,
        panel: SearchFocus,
    ) -> u16 {
        let steady = if self.focus == panel { 1000 } else { 0 };
        match transition {
            SearchFocusTransition::Instant => steady,
            SearchFocusTransition::Slide => {
                if self.focus_ring.settled() {
                    steady
                } else if panel == self.focus {
                    self.focus_ring.eased_in_out()
                } else if panel == self.prev_focus {
                    1000_u16.saturating_sub(self.focus_ring.eased_in_out())
                } else {
                    0
                }
            }
        }
    }

    /// Prompt 段焦点复位：落 query 段、下拉收起（进入 / 回 prompt 时调用）。
    fn reset_prompt_seg(&mut self) {
        self.prompt_seg = PromptSegment::Query;
        self.seg_open = false;
        self.seg_sel = 0;
        self.reveal_seg = None;
        self.seg_reveal = Transition::new(self.ring_ticks);
    }

    /// 当前 Prompt 段（仅 `focus == Prompt` 有意义）。
    pub fn prompt_seg(&self) -> PromptSegment {
        self.prompt_seg
    }

    /// 渲染用：focus 真在 Prompt 时给出当前段，否则 `None`（不画段高亮 / 下拉）。
    pub fn prompt_focus(&self) -> Option<PromptSegment> {
        (self.focus == SearchFocus::Prompt).then_some(self.prompt_seg)
    }

    /// chip 段下拉是否展开。
    pub fn seg_open(&self) -> bool {
        self.seg_open
    }

    /// 下拉高亮行下标。
    pub fn seg_sel(&self) -> usize {
        self.seg_sel
    }

    /// chip 下拉展开动画的缓动进度（千分比；渲染按它从上往下展开下拉高度）。
    pub fn seg_reveal(&self) -> u16 {
        self.seg_reveal.eased()
    }

    /// 下拉归属的 chip 段（与 `focus` 解耦）：展开 / 收起动画期都是 `Some`，渲染据此画哪个
    /// chip 的下拉——切到 query 后仍能把上一个 chip 的收起动画画完。
    pub fn reveal_seg(&self) -> Option<PromptSegment> {
        self.reveal_seg
    }

    /// 下拉是否仍需渲染：展开动画进行中或已展开（收起动画播完归零才停画）。
    /// 与 `seg_open`（逻辑开关）解耦——收起后视觉收尾期仍 `true`，让 collapse 动画放完。
    pub fn dropdown_active(&self) -> bool {
        self.seg_reveal.active()
    }

    /// 在 `seg` 处展开下拉：reveal 归属切到 `seg`、动画**从零重播展开**——故每次 focus 到
    /// chip(含 chip↔chip 切换)都重新展开,不是「已开就不动」。
    fn open_reveal(&mut self, seg: PromptSegment) {
        self.reveal_seg = Some(seg);
        self.seg_reveal = Transition::expanding(self.ring_ticks);
    }

    /// 收起当前下拉:保留 reveal 归属(让收起动画画完)、动画从当前进度退场到零。
    fn close_reveal(&mut self) {
        self.seg_reveal.leave();
    }

    /// 把 Prompt 焦点移到某段：chip 段（Source/Kind）展开下拉并把高亮落在 `sel`，
    /// Query 段收起。展开 / 收起各驱动 reveal 动画进 / 退场。
    pub fn set_prompt_seg(&mut self, seg: PromptSegment, sel: usize) {
        self.prompt_seg = seg;
        self.seg_open = seg != PromptSegment::Query;
        self.seg_sel = sel;
        if self.seg_open {
            self.open_reveal(seg);
        } else {
            self.close_reveal();
        }
    }

    /// 收起当前 chip 下拉（选定塌回 / Esc 收起），仍停在该段；播收起动画。
    pub fn close_seg(&mut self) {
        self.seg_open = false;
        self.close_reveal();
    }

    /// 重新展开当前 chip 下拉（塌回后再 Enter），高亮落 `sel`，重播展开动画。
    pub fn open_seg(&mut self, sel: usize) {
        self.seg_open = true;
        self.seg_sel = sel;
        self.open_reveal(self.prompt_seg);
    }

    /// 下拉高亮行设为 `sel`（调用方已钳到列表范围内）。
    pub fn set_seg_sel(&mut self, sel: usize) {
        self.seg_sel = sel;
    }

    /// 推进布局形变、焦点环、当前 detail 栈滑动各一拍（同帧驱动，由主循环 tick 调用）。
    pub(crate) fn tick(&mut self) {
        self.active.tick();
        self.focus_ring.tick();
        self.seg_reveal.tick();
        // loading spinner 帧恒推进（wrapping 防溢出 lint;视觉上一直旋转）。
        self.spinner = self.spinner.wrapping_add(1);
        if let Some(kr) = self.active_results_mut() {
            kr.detail.tick();
        }
    }

    /// 进入 Search 布局态：挑默认搜索 source + 确保其会话存在，焦点回 prompt。
    ///
    /// 默认 source = 首个 `searchable` 非空的 source（按 `name()` 定序去抖，多 source 下确定）；
    /// 默认 kind = 该 source 过 kind 白名单后的首项。已有 source 则保留（切回恢复），仅复位焦点。
    /// 无可搜索 source 时 `source` 留 `None`（空态由渲染层提示）。
    ///
    /// # Params:
    ///   - `caps`: 各 source 能力声明（决定哪些 source 可搜、默认 kind）
    pub(crate) fn enter(&mut self, caps: &FxHashMap<SourceKind, ChannelCaps>) {
        // 焦点落 prompt 且焦点环复位：布局形变本身已是进场动画，无需叠加面板内滑动。
        self.focus = SearchFocus::Prompt;
        self.prev_focus = SearchFocus::Prompt;
        self.focus_ring = Transition::new(self.ring_ticks);
        self.reset_prompt_seg();
        if self.source.is_some() {
            return;
        }
        // 默认 source = 下拉首项(白名单定序;不设白名单则 name() 字典序去抖)。
        let options = self.source_options(caps);
        let Some(source) = options.first().copied() else {
            return;
        };
        // 白名单滤空的防呆回退只在进场告警一次——下拉数据每帧重算,不在那儿刷日志。
        if !self.whitelist.sources.is_empty()
            && !self
                .whitelist
                .sources
                .iter()
                .any(|name| name == source.name())
        {
            mineral_log::warn!(
                target: "tui",
                "search source 白名单与已加载 source 无交集,回退全量"
            );
        }
        if !self.whitelist.kinds.is_empty()
            && caps.get(&source).is_some_and(|channel_caps| {
                search_whitelist::whitelisted_kinds(&self.whitelist, channel_caps).is_empty()
            })
        {
            mineral_log::warn!(
                target: "tui",
                "search kind 白名单与各 source 可搜类型无交集,回退全量"
            );
        }
        let kind = search_whitelist::default_kind(&self.whitelist, caps, source);
        self.source = Some(source);
        self.sessions
            .entry(source)
            .or_insert_with(|| SearchSession::new(kind));
    }

    /// 当前 source 的搜索会话（只读）。
    pub fn current(&self) -> Option<&SearchSession> {
        self.source.and_then(|s| self.sessions.get(&s))
    }

    /// 当前 source 的搜索会话（可变；打字 / 翻页写入用）。
    pub fn current_mut(&mut self) -> Option<&mut SearchSession> {
        match self.source {
            Some(src) => self.sessions.get_mut(&src),
            None => None,
        }
    }

    /// 当前 source × 当前 kind 的结果桶（只读）：渲染结果列/详情的直达入口。
    pub fn active_results(&self) -> Option<&KindResults> {
        self.current().and_then(SearchSession::kind_results)
    }

    /// 当前 source × 当前 kind 的结果桶（可变）：光标移动 / detail 派发写入。
    pub fn active_results_mut(&mut self) -> Option<&mut KindResults> {
        self.current_mut().and_then(SearchSession::kind_results_mut)
    }

    /// 遍历所有会话保留的结果桶；实体详情按完整 ID 配对，不受当前 source / kind 影响。
    pub(crate) fn retained_results_mut(&mut self) -> impl Iterator<Item = &mut KindResults> {
        self.sessions
            .values_mut()
            .flat_map(SearchSession::retained_results_mut)
    }

    /// 按 source 找会话（可变）：搜索回包按事件自带 source 配对（可能非当前 source）。
    pub fn session_for_mut(&mut self, source: SourceKind) -> Option<&mut SearchSession> {
        self.sessions.get_mut(&source)
    }

    /// 切 source：切到目标 source、确保会话存在（新建用其过 kind 白名单后的首项，已有会话则
    /// 沿用记住的 kind）。
    pub fn switch_source(&mut self, source: SourceKind, caps: &FxHashMap<SourceKind, ChannelCaps>) {
        // 已有会话时当帧直接换活跃桶(detail 头图立即变),计一次选中变化走防抖。
        self.last_sel_change = Instant::now();
        self.source = Some(source);
        if self.sessions.contains_key(&source) {
            return;
        }
        let kind = search_whitelist::default_kind(&self.whitelist, caps, source);
        self.sessions.insert(source, SearchSession::new(kind));
    }

    /// 可搜索的 source 列表(白名单定序过滤,规则见 [`search_whitelist::source_options`]):
    /// chip 下拉 / 段切换 / 渲染三处共用同一定序,保证 `seg_sel` 下标与展示一致。
    pub fn source_options(&self, caps: &FxHashMap<SourceKind, ChannelCaps>) -> Vec<SourceKind> {
        search_whitelist::source_options(&self.whitelist, caps)
    }

    /// 当前 source 支持的 kind 列表(过 kind 白名单,保白名单顺序):chip 下拉 / 段切换 / 渲染共用。
    ///
    /// 交集为空回退 searchable 全量——正常流程下交集空的 source 已被 [`Self::source_options`]
    /// 隐藏,能走到这只有「kind 配置滤光一切被整体忽略」的回退态,此处同步回退保持一致。
    pub fn kind_options(&self, caps: &FxHashMap<SourceKind, ChannelCaps>) -> Vec<SearchKind> {
        let Some(channel_caps) = self.source.and_then(|source| caps.get(&source)) else {
            return Vec::new();
        };
        let intersection = search_whitelist::whitelisted_kinds(&self.whitelist, channel_caps);
        if intersection.is_empty() {
            return channel_caps.searchable().clone();
        }
        intersection
    }

    /// 切 kind：换当前会话 kind。
    ///
    /// # Return:
    ///   是否需要自动搜（目标 kind 无缓存 + query 非空）。
    pub fn select_kind(&mut self, kind: SearchKind) -> bool {
        let Some(session) = self.current_mut() else {
            return false;
        };
        session.set_kind(kind);
        let auto_search = !session.has_current_results() && !session.input.is_empty();
        // 目标 kind 有缓存桶时当帧换 detail 头图,计一次选中变化走防抖。
        self.last_sel_change = Instant::now();
        auto_search
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::{ChannelCaps, Page};
    use mineral_model::{SearchKind, SourceKind};
    use mineral_task::SearchPayload;
    use rustc_hash::FxHashMap;

    use super::{PromptSegment, SearchFocus, SearchPage, SearchWhitelist};

    /// 造一个落在 NETEASE、给定 searchable 的 caps 表。
    fn caps_with(kinds: Vec<SearchKind>) -> FxHashMap<SourceKind, ChannelCaps> {
        let mut caps = FxHashMap::default();
        caps.insert(
            SourceKind::NETEASE,
            ChannelCaps::builder()
                .searchable(kinds)
                .playlist_edit(false)
                .artist_sections(music_sections())
                .build(),
        );
        caps
    }

    /// 两区皆有的 artist 分区(测试夹具:音乐源形态)。
    fn music_sections() -> mineral_channel_core::ArtistSections {
        mineral_channel_core::ArtistSections::new(vec![
            mineral_channel_core::ArtistSectionKind::TopSongs,
            mineral_channel_core::ArtistSectionKind::Albums,
        ])
    }

    /// loading 三态:提交置位 → current_loading 真;首页到货(含 0 条)清位 → 转 empty。
    #[test]
    fn loading_set_on_submit_cleared_on_first_page() -> color_eyre::Result<()> {
        let caps = caps_with(vec![SearchKind::Song]);
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1);
        rs.enter(&caps);
        rs.mark_loading(SearchKind::Song);
        assert!(rs.current_loading(), "提交后当前 kind 在飞 → loading");
        if let Some(session) = rs.current_mut() {
            // 首页到货 0 条:仍算「搜完了」,清 loading(渲染层转 no results)。
            session.apply_page(
                SearchKind::Song,
                SearchPayload::Songs(Vec::new()),
                Page::default(),
                /*has_more*/ None,
            );
        }
        assert!(!rs.current_loading(), "首页到货 → 清 loading");
        Ok(())
    }

    /// 换词作废(clear_results)清 loading,避免旧词的 in-flight 残留显假 loading。
    #[test]
    fn loading_cleared_on_clear_results() -> color_eyre::Result<()> {
        let caps = caps_with(vec![SearchKind::Song]);
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1);
        rs.enter(&caps);
        rs.mark_loading(SearchKind::Song);
        assert!(rs.current_loading());
        if let Some(session) = rs.current_mut() {
            session.clear_results();
        }
        assert!(!rs.current_loading(), "换词 clear_results 清 loading");
        Ok(())
    }

    /// spinner 帧计数随 tick 单调 +1(渲染层据此取旋转帧,故 loading 占位会持续旋转)。
    #[test]
    fn spinner_counter_advances_with_tick() {
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1);
        let c0 = rs.spinner_counter();
        rs.tick();
        rs.tick();
        assert_eq!(rs.spinner_counter(), c0 + 2, "每 tick spinner 计数 +1");
    }

    /// 进入时挑首个 searchable source、kind 落到该 source searchable 首项、焦点回 prompt。
    #[test]
    fn enter_picks_first_searchable_source() -> color_eyre::Result<()> {
        let caps = caps_with(vec![SearchKind::Album, SearchKind::Song]);
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1);
        rs.enter(&caps);

        assert_eq!(
            rs.source,
            Some(SourceKind::NETEASE),
            "落到唯一 searchable source"
        );
        assert_eq!(rs.focus, SearchFocus::Prompt, "焦点回 prompt");
        let session = rs
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("入会后应有当前会话"))?;
        assert_eq!(session.kind, SearchKind::Album, "kind 落到 searchable 首项");
        Ok(())
    }

    /// 无 searchable source（caps 全空）时 source 留 None（空态）。
    #[test]
    fn enter_with_no_searchable_source_stays_none() {
        let caps = FxHashMap::<SourceKind, ChannelCaps>::default();
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1);
        rs.enter(&caps);
        assert_eq!(rs.source, None, "无可搜索 source → 空态");
    }

    /// 造多 source 的 caps 表:每项 `(source, searchable)`。
    fn caps_multi(
        entries: Vec<(SourceKind, Vec<SearchKind>)>,
    ) -> FxHashMap<SourceKind, ChannelCaps> {
        let mut caps = FxHashMap::default();
        for (source, kinds) in entries {
            caps.insert(
                source,
                ChannelCaps::builder()
                    .searchable(kinds)
                    .playlist_edit(false)
                    .artist_sections(music_sections())
                    .build(),
            );
        }
        caps
    }

    /// source 白名单:顺序即下拉顺序(可与字典序相反)、未列出即隐藏、没加载的名字静默跳过。
    #[test]
    fn source_options_follow_whitelist_order_and_hide_unlisted() {
        let caps = caps_multi(vec![
            (SourceKind::BILIBILI, vec![SearchKind::Song]),
            (SourceKind::NETEASE, vec![SearchKind::Song]),
            (SourceKind::LOCAL, vec![SearchKind::Song]),
        ]);
        let rs =
            SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1).with_whitelist(SearchWhitelist {
                sources: vec![
                    "netease".to_owned(),
                    "ghost".to_owned(),
                    "bilibili".to_owned(),
                ],
                kinds: Vec::new(),
            });
        assert_eq!(
            rs.source_options(&caps),
            vec![SourceKind::NETEASE, SourceKind::BILIBILI],
            "配置序 netease→bilibili(逆字典序),ghost 静默跳过,local 未列出隐藏"
        );
    }

    /// source 白名单滤到空(全是没加载的名字)→ 回退全量字典序,不让搜索页变空壳。
    #[test]
    fn source_whitelist_all_unknown_falls_back_to_full() {
        let caps = caps_multi(vec![
            (SourceKind::NETEASE, vec![SearchKind::Song]),
            (SourceKind::BILIBILI, vec![SearchKind::Song]),
        ]);
        let rs =
            SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1).with_whitelist(SearchWhitelist {
                sources: vec!["ghost".to_owned()],
                kinds: Vec::new(),
            });
        assert_eq!(
            rs.source_options(&caps),
            vec![SourceKind::BILIBILI, SourceKind::NETEASE],
            "白名单全落空 → 忽略配置回退字典序全量"
        );
    }

    /// kind 白名单:与当前 source 的 searchable 求交、保配置顺序;交集空的 source 从
    /// source 下拉整体消失。
    #[test]
    fn kind_whitelist_orders_and_hides_kindless_source() -> color_eyre::Result<()> {
        let caps = caps_multi(vec![
            (
                SourceKind::NETEASE,
                vec![SearchKind::Song, SearchKind::Album, SearchKind::Artist],
            ),
            (SourceKind::BILIBILI, vec![SearchKind::User]),
        ]);
        let mut rs =
            SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1).with_whitelist(SearchWhitelist {
                sources: Vec::new(),
                kinds: vec![SearchKind::Album, SearchKind::Song],
            });
        assert_eq!(
            rs.source_options(&caps),
            vec![SourceKind::NETEASE],
            "bilibili 只可搜 user、与 kind 白名单交集空 → 隐藏"
        );
        rs.enter(&caps);
        assert_eq!(
            rs.kind_options(&caps),
            vec![SearchKind::Album, SearchKind::Song],
            "kind 按配置序求交,artist 未列出隐藏"
        );
        let session = rs
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("入会后应有当前会话"))?;
        assert_eq!(session.kind, SearchKind::Album, "默认 kind 落到过滤后首项");
        Ok(())
    }

    /// kind 白名单把所有 source 都滤没了 → 整体忽略 kind 配置回退全量(source 与 kind 一致回退)。
    #[test]
    fn kind_whitelist_wiping_everything_is_ignored() {
        let caps = caps_multi(vec![(SourceKind::NETEASE, vec![SearchKind::Song])]);
        let mut rs =
            SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1).with_whitelist(SearchWhitelist {
                sources: Vec::new(),
                kinds: vec![SearchKind::Playlist],
            });
        assert_eq!(
            rs.source_options(&caps),
            vec![SourceKind::NETEASE],
            "kind 配置滤光一切 → 忽略之,source 全量"
        );
        rs.enter(&caps);
        assert_eq!(
            rs.kind_options(&caps),
            vec![SearchKind::Song],
            "同一回退下 kind 也给 searchable 全量,不留空下拉"
        );
    }

    /// 进入时默认 source 跟白名单首位(而非字典序首位);switch_source 的默认 kind 也过白名单。
    #[test]
    fn enter_and_switch_respect_whitelist() -> color_eyre::Result<()> {
        let caps = caps_multi(vec![
            (
                SourceKind::BILIBILI,
                vec![SearchKind::Song, SearchKind::Album],
            ),
            (
                SourceKind::NETEASE,
                vec![SearchKind::Song, SearchKind::Album],
            ),
        ]);
        let mut rs =
            SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1).with_whitelist(SearchWhitelist {
                sources: vec!["netease".to_owned(), "bilibili".to_owned()],
                kinds: vec![SearchKind::Album],
            });
        rs.enter(&caps);
        assert_eq!(
            rs.source,
            Some(SourceKind::NETEASE),
            "默认 source = 白名单首位,非字典序的 bilibili"
        );
        let session = rs
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("入会后应有当前会话"))?;
        assert_eq!(session.kind, SearchKind::Album, "默认 kind 过白名单");
        rs.switch_source(SourceKind::BILIBILI, &caps);
        let session = rs
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("切源后应有当前会话"))?;
        assert_eq!(
            session.kind,
            SearchKind::Album,
            "首次切入 bilibili:kind 落到白名单过滤后的首项,而非 searchable 首项 song"
        );
        Ok(())
    }

    /// `set_focus`：切到面板记住 `last_panel`、armed 焦点环；回 prompt 不改 `last_panel`；
    /// 同焦点为 no-op（不重置已 settle 的环）。
    #[test]
    fn set_focus_tracks_last_panel_and_arms_ring() {
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 4);
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

        for _ in 0..4 {
            rs.tick();
        }
        assert!(rs.focus_ring.settled(), "推满后环 settle");
        rs.set_focus(SearchFocus::Prompt);
        assert!(rs.focus_ring.settled(), "同焦点不重新 arm");
    }

    /// chip 下拉收起播 collapse 动画:close_seg 后 `seg_open` 已假,但 `dropdown_active` 仍真
    /// （视觉收尾期继续画着往回收），tick 到 settle 才归零停画。
    #[test]
    fn dropdown_collapse_animates_after_close() {
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 4);
        rs.set_prompt_seg(PromptSegment::Kind, 0);
        for _ in 0..8 {
            rs.tick();
        }
        assert!(rs.dropdown_active(), "展开 settle 后仍需渲染");
        rs.close_seg();
        assert!(!rs.seg_open(), "逻辑上已收起");
        assert!(rs.dropdown_active(), "但收起动画进行中,仍画着往回收");
        for _ in 0..8 {
            rs.tick();
        }
        assert!(!rs.dropdown_active(), "收起动画播完归零,停止渲染");
    }

    /// chip↔chip 切换重播展开动画;切到 query 收起动画仍画(reveal 归属保留上一个 chip)。
    #[test]
    fn dropdown_reanimates_on_chip_switch_then_query_collapse() {
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 4);
        rs.set_prompt_seg(PromptSegment::Kind, 0);
        for _ in 0..8 {
            rs.tick();
        }
        assert_eq!(rs.reveal_seg(), Some(PromptSegment::Kind), "归属在 kind");
        assert_eq!(rs.seg_reveal(), 1000, "kind 下拉已满展开");
        // chip→chip:归属切到 source、展开动画从零重播(不是「已开就不动」)。
        rs.set_prompt_seg(PromptSegment::Source, 0);
        assert_eq!(
            rs.reveal_seg(),
            Some(PromptSegment::Source),
            "归属切到 source"
        );
        assert!(rs.seg_reveal() < 1000, "切 chip 重播展开,非保持满值");
        for _ in 0..8 {
            rs.tick();
        }
        // chip→query:收起,归属仍保留 source(把收起动画画完)。
        rs.set_prompt_seg(PromptSegment::Query, 0);
        assert_eq!(
            rs.reveal_seg(),
            Some(PromptSegment::Source),
            "切 query 后归属保留,收起动画照画"
        );
        assert!(rs.dropdown_active(), "收起动画进行中仍渲染");
        for _ in 0..8 {
            rs.tick();
        }
        assert!(!rs.dropdown_active(), "收起播完停画");
    }
}

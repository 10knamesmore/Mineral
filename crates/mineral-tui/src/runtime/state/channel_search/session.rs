//! 单个 source 的搜索词编辑、在飞标记与按 kind 保存的分页结果。

use mineral_channel_core::{ArtistSections, Page};
use mineral_model::SearchKind;
use mineral_task::SearchPayload;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::runtime::line_input::{InputRequest, LineInput};

use super::KindResults;

/// 单个 source 的搜索会话：当前 kind + 输入词（source 级共享）+ per-kind 结果桶。
///
/// 切 source 切的是整个会话（各 source 独立、切回恢复）；`query` 改变作废本会话全部 kind 桶
/// （旧词的结果整体过期），切 kind 只换当前桶、其余桶保留（同词切回不重搜）。
pub struct SearchSession {
    /// 当前选中的 kind（kind chip 下拉切换；每 source 记住自己的选择）。
    pub kind: SearchKind,

    /// token prompt 输入词 + 文本光标（通用 [`LineInput`]；随会话走，切 source 切回各自保留）。
    pub(super) input: LineInput,

    /// per-kind 结果桶（query 改变即整体作废）。
    by_kind: FxHashMap<SearchKind, KindResults>,

    /// 首页搜索在飞中的 kind（提交即置位、首页到货清位）。渲染层据此把「正在搜」与「搜到
    /// 0 条」「尚未搜索」三态分开。读失败无事件 → 不清 → 持续显 loading,重 Enter 重提交
    /// （与 spec「读失败=停留 loading + 驻留重试」一致）。
    in_flight: FxHashSet<SearchKind>,
}

impl SearchSession {
    /// 新会话：给定默认 kind、空输入、无结果桶。
    pub(super) fn new(kind: SearchKind) -> Self {
        Self {
            kind,
            input: LineInput::new(),
            by_kind: FxHashMap::default(),
            in_flight: FxHashSet::default(),
        }
    }

    /// 标记某 kind 首页搜索在飞（提交时置位）。
    pub(crate) fn mark_in_flight(&mut self, kind: SearchKind) {
        self.in_flight.insert(kind);
    }

    /// 某 kind 是否首页搜索在飞（渲染层据此区分 loading↔empty）。
    pub(crate) fn is_loading(&self, kind: SearchKind) -> bool {
        self.in_flight.contains(&kind)
    }

    /// 当前 kind 的结果桶（只读）；未搜该 kind 为 `None`。
    pub fn kind_results(&self) -> Option<&KindResults> {
        self.by_kind.get(&self.kind)
    }

    /// 当前 kind 的结果桶（可变）。
    pub fn kind_results_mut(&mut self) -> Option<&mut KindResults> {
        self.by_kind.get_mut(&self.kind)
    }

    /// 遍历本会话保留的结果桶，供详情回包更新非当前 kind 的帧。
    pub(super) fn retained_results_mut(&mut self) -> impl Iterator<Item = &mut KindResults> {
        self.by_kind.values_mut()
    }

    /// 切当前 kind（不动其它桶，同词切回复用）。
    pub fn set_kind(&mut self, kind: SearchKind) {
        self.kind = kind;
    }

    /// 当前 kind 是否已有结果桶（切 kind 后据此决定要不要自动搜）。
    pub fn has_current_results(&self) -> bool {
        self.by_kind.contains_key(&self.kind)
    }

    /// 当前输入词（只读）。
    pub fn query(&self) -> &str {
        self.input.text()
    }

    /// 测试构造：一次性灌入整段 query、光标落词尾、作废所有 kind 桶（生产路径是逐字符
    /// [`Self::push_query_char`]，故仅测试需要这个整段入口）。
    #[cfg(test)]
    pub fn set_query(&mut self, q: impl Into<String>) {
        self.input.set_text(q);
        self.clear_results();
    }

    /// 在光标处插入字符、光标右移一格，并作废所有 kind 的结果与加载状态。
    pub fn push_query_char(&mut self, c: char) {
        self.input.apply(InputRequest::Insert(c));
        self.clear_results();
    }

    /// 退格：删光标前一字符。光标 > 0 才删（删了返回 `true` 并作废结果与加载状态），词首
    /// 返回 `false`（键路由据此知道「无字可删」而静默吞键）。
    pub fn pop_query_char(&mut self) -> bool {
        let changed = self.input.apply(InputRequest::DeletePrev);
        if changed {
            self.clear_results();
        }
        changed
    }

    /// 文本光标编辑委托给通用 [`LineInput`]；默认界面 `/` 模糊框亦用它，行为一致。
    /// 文本光标左移一格（钳词首）。
    pub fn cursor_left(&mut self) {
        self.input.apply(InputRequest::Left);
    }

    /// 文本光标右移一格（钳词尾）。
    pub fn cursor_right(&mut self) {
        self.input.apply(InputRequest::Right);
    }

    /// 文本光标跳词首。
    pub fn cursor_home(&mut self) {
        self.input.apply(InputRequest::Home);
    }

    /// 文本光标跳词尾。
    pub fn cursor_end(&mut self) {
        self.input.apply(InputRequest::End);
    }

    /// query 以光标为界切两段 `(光标前, 光标后)`（渲染光标块用；光标恒落 char 边界）。
    pub fn query_split(&self) -> (&str, &str) {
        self.input.split()
    }

    /// 编辑搜索词或显式重新提交时，作废全部 kind 的结果与加载状态。
    /// 新请求由提交入口重新标记；只编辑未提交时保持未搜索状态。
    pub fn clear_results(&mut self) {
        mineral_log::debug!(target: "tui", results = self.by_kind.len(), pending = self.in_flight.len(), "invalidate search results and loading");
        self.by_kind.clear();
        self.in_flight.clear();
    }

    /// 把一页结果落进 `by_kind[kind]`：首页新建桶、翻页 append 既有桶。
    ///
    /// # Params:
    ///   - `kind`: 事件自带的 kind（决定存哪个桶）
    ///   - `payload`: 结果载荷
    ///   - `page`: 分页参数（`offset == 0` 为首页）
    ///   - `has_more`: 源的显式翻页信号（`None` 回退短页推断）
    pub fn apply_page(
        &mut self,
        kind: SearchKind,
        payload: SearchPayload,
        page: Page,
        has_more: Option<bool>,
    ) {
        if page.offset == 0 {
            // 首页到货:清 loading（即便 0 条也算「搜完了」→ 渲染层转 no results）。
            self.in_flight.remove(&kind);
            self.by_kind
                .insert(kind, KindResults::first_page(payload, page.limit, has_more));
        } else if let Some(bucket) = self.by_kind.get_mut(&kind) {
            bucket.append_page(payload, page.limit, has_more);
        }
    }

    /// 按 caps 落定某 kind 桶所属源的 artist 可用分区（首页到货后调,持 caps）。
    /// 桶不存在(未搜该 kind)则无操作。
    pub fn apply_sections(&mut self, kind: SearchKind, sections: ArtistSections) {
        if let Some(bucket) = self.by_kind.get_mut(&kind) {
            bucket.apply_sections(sections);
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::Page;
    use mineral_model::SearchKind;
    use mineral_task::SearchPayload;

    use crate::test_support::endserenading;

    use super::SearchSession;

    /// 5 条歌曲一页（配 limit=5 即满页；endserenading 上限 10 条）。
    fn full_page() -> SearchPayload {
        SearchPayload::Songs(endserenading(5))
    }

    /// 改 query 作废全部 kind 桶（旧词结果整体过期）。
    #[test]
    fn query_change_drops_buckets() {
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(
            SearchKind::Song,
            full_page(),
            Page::default(),
            /*has_more*/ None,
        );
        assert!(s.kind_results().is_some(), "搜后有桶");
        s.push_query_char('x');
        assert!(s.kind_results().is_none(), "改 query 作废桶");
    }

    /// 文本光标:插入落在光标处、Left/Right 钳边、退格删光标前一字符、词首退格 no-op。
    #[test]
    fn prompt_cursor_edits_at_position() {
        let mut s = SearchSession::new(SearchKind::Song);
        for c in "ab".chars() {
            s.push_query_char(c);
        }
        assert_eq!(s.query_split(), ("ab", ""), "插入后光标在词尾");
        s.cursor_left();
        assert_eq!(s.query_split(), ("a", "b"), "左移一格落 a|b");
        s.push_query_char('X');
        assert_eq!(s.query(), "aXb", "插入落在光标处而非词尾");
        assert_eq!(s.query_split(), ("aX", "b"), "插入后光标停在新字符之后");
        assert!(s.pop_query_char(), "退格删光标前的 X 返回 true");
        assert_eq!(s.query(), "ab", "退格删掉的是光标前一字符");
        s.cursor_home();
        assert!(!s.pop_query_char(), "词首退格 no-op 返回 false");
        assert_eq!(s.query(), "ab", "词首退格不改 query");
        s.cursor_end();
        s.cursor_right();
        assert_eq!(s.query_split(), ("ab", ""), "右移越界钳词尾");
    }

    /// 光标操作和词首退格不改变文本，不能取消已提交搜索的加载状态。
    #[test]
    fn cursor_moves_and_empty_backspace_keep_pending_search() {
        let mut s = SearchSession::new(SearchKind::Song);
        s.set_query("query");
        s.mark_in_flight(SearchKind::Song);
        s.cursor_left();
        s.cursor_right();
        s.cursor_home();
        assert!(!s.pop_query_char());
        s.cursor_end();
        assert_eq!(s.query(), "query");
        assert!(
            s.is_loading(SearchKind::Song),
            "未改文本时保留已提交请求的加载状态"
        );
    }

    /// 多字节(CJK)光标:byte 偏移按 char 边界,`query_split` 不切坏字符。
    #[test]
    fn prompt_cursor_multibyte_safe() {
        let mut s = SearchSession::new(SearchKind::Song);
        for c in "周杰伦".chars() {
            s.push_query_char(c);
        }
        s.cursor_left();
        assert_eq!(s.query_split(), ("周杰", "伦"), "光标落在 char 边界");
        s.push_query_char('a');
        assert_eq!(s.query(), "周杰a伦", "多字节中间插入不切坏字符");
    }
}

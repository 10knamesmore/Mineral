//! 保存列表的续页进度，配对请求与回包，并按来源信号或短页判断是否收齐。

use mineral_channel_core::Page;

/// 从已收到的首页建立分页状态，供搜索结果桶与艺人专辑列表共同使用。
///
/// 页大小沿用首页请求；同一列表在消费成功或失败回包前只登记一页续页。
#[derive(Clone)]
pub(super) struct ListPagination {
    /// 下一页请求；offset 按每页 limit 推进，不能按实际条数推进，
    /// 否则页码型来源的短页会导致页号折回或跳页。
    next_page: Page,

    /// 已发出、尚未消费成功或失败回包的续页；scheduler 完成任务不会清除此状态。
    pending_page: Option<Page>,

    /// 来源明确没有下一页，或缺少显式信号且收到短页；为真时停止预取。
    exhausted: bool,
}

impl ListPagination {
    /// 从首页请求与实际条数建立后续分页进度。
    ///
    /// # Params:
    ///   - `loaded`: 首页实际返回条数。
    ///   - `page`: 已接收的首页请求，后续 offset 按其 limit 推进。
    ///   - `has_more`: 来源的显式翻页信号；`None` 时按短页推断是否收齐。
    pub(super) fn first_page(loaded: usize, page: Page, has_more: Option<bool>) -> Self {
        Self {
            next_page: Page::new(page.offset.saturating_add(page.limit), page.limit),
            pending_page: None,
            exhausted: page_exhausts(loaded, page.limit, has_more),
        }
    }

    /// 登记待收取的下一页；正在等待回包或已收齐时返回 `None`。
    pub(super) fn request_next_page(&mut self) -> Option<Page> {
        if self.exhausted || self.is_loading() {
            return None;
        }
        let page = self.next_page;
        self.pending_page = Some(page);
        mineral_log::debug!(target: "tui", ?page, "request next list page");
        Some(page)
    }

    /// 只接受同时匹配待收取请求与预期进度的续页，并将 offset 推进一页。
    ///
    /// # Params:
    ///   - `loaded`: 本页实际返回条数。
    ///   - `page`: 回包对应的分页参数。
    ///   - `has_more`: 来源的显式翻页信号；`None` 时按短页推断是否收齐。
    ///
    /// # Return:
    ///   接受回包返回 `true`，调用方随后追加条目；拒绝时返回 `false` 且不改变状态。
    pub(super) fn accept_page(
        &mut self,
        loaded: usize,
        page: Page,
        has_more: Option<bool>,
    ) -> bool {
        if self.pending_page != Some(page) || self.next_page != page {
            mineral_log::debug!(target: "tui", ?page, pending = ?self.pending_page, expected = ?self.next_page, "ignore unexpected list page");
            return false;
        }
        self.pending_page = None;
        self.next_page.offset = page.offset.saturating_add(page.limit);
        self.exhausted = page_exhausts(loaded, page.limit, has_more);
        mineral_log::debug!(target: "tui", ?page, loaded, next_offset = self.next_page.offset, exhausted = self.exhausted, "accept list page");
        true
    }

    /// 释放匹配的失败或取消请求，保留分页进度以便重试同一页。
    ///
    /// # Params:
    ///   - `page`: 失败或取消任务携带的分页参数；不匹配待收取请求时忽略。
    pub(super) fn fail_page(&mut self, page: Page) {
        if self.pending_page == Some(page) {
            self.pending_page = None;
            mineral_log::debug!(target: "tui", ?page, "release failed list page for retry");
        }
    }

    /// 是否已收齐列表，供调用方停止预取并显示总条数。
    pub(super) fn exhausted(&self) -> bool {
        self.exhausted
    }

    /// 是否已登记续页请求且尚未消费成功或失败回包。
    pub(super) fn is_loading(&self) -> bool {
        self.pending_page.is_some()
    }

    /// 返回预期进度，供回归断言已收齐时仍按页大小推进 offset。
    #[cfg(test)]
    pub(super) fn expected_page(&self) -> Page {
        self.next_page
    }
}

/// 优先采用来源的显式信号；缺少信号时，实际条数少于请求页大小即视为收齐。
///
/// # Params:
///   - `loaded`: 本页实际返回条数。
///   - `limit`: 请求页大小。
///   - `has_more`: 来源的显式翻页信号。
fn page_exhausts(loaded: usize, limit: u32, has_more: Option<bool>) -> bool {
    match has_more {
        Some(more) => !more,
        None => u32::try_from(loaded).unwrap_or(u32::MAX) < limit,
    }
}

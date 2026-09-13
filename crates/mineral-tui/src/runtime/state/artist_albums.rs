//! 累积艺人专辑列表，并跟踪该列表的续页请求与加载状态。

use mineral_channel_core::Page;
use mineral_model::Album;

use super::pagination::ListPagination;

/// 保存详情帧已收到首页的艺人专辑列表；空列表表示已收取页没有可展示专辑，
/// 是否结束由分页信号决定。
///
/// 详情帧在首页到货时构造，后续只追加已请求的页；随帧保留或销毁，动画副本可克隆。
#[derive(Clone)]
pub(crate) struct ArtistAlbums {
    /// 已接受的各页专辑，按回包顺序累积。
    albums: Vec<Album>,

    /// 首页确定的分页进度与尚未消费回包的续页请求。
    pagination: ListPagination,
}

impl ArtistAlbums {
    /// 用已收到的首页创建列表与后续分页进度。
    ///
    /// # Params:
    ///   - `albums`: 首页实际返回的专辑。
    ///   - `page`: 首页请求的分页参数，后续 offset 按其 limit 推进。
    ///   - `has_more`: 来源的显式翻页信号；`None` 时按短页推断是否收齐。
    pub(crate) fn first_page(albums: Vec<Album>, page: Page, has_more: Option<bool>) -> Self {
        Self {
            pagination: ListPagination::first_page(albums.len(), page, has_more),
            albums,
        }
    }

    /// 已接受的所有专辑，保持列表展示顺序。
    pub(crate) fn items(&self) -> &[Album] {
        &self.albums
    }

    /// 登记下一页请求；正在等待回包或已收齐时返回 `None`。
    pub(crate) fn request_next_page(&mut self) -> Option<Page> {
        self.pagination.request_next_page()
    }

    /// 追加已请求且符合预期进度的续页；拒绝重复、超前或未请求的页。
    ///
    /// # Params:
    ///   - `albums`: 本页实际返回的专辑。
    ///   - `page`: 回包对应的分页参数。
    ///   - `has_more`: 来源的显式翻页信号；`None` 时按短页推断是否收齐。
    ///
    /// # Return:
    ///   接受并追加返回 `true`；拒绝时返回 `false`，列表与分页进度不变。
    pub(crate) fn append_page(
        &mut self,
        albums: Vec<Album>,
        page: Page,
        has_more: Option<bool>,
    ) -> bool {
        if !self.pagination.accept_page(albums.len(), page, has_more) {
            return false;
        }
        self.albums.extend(albums);
        true
    }

    /// 释放匹配的失败续页，保留列表与分页进度以便重试。
    ///
    /// # Params:
    ///   - `page`: 失败或取消任务携带的分页参数；不匹配待收取请求时忽略。
    pub(crate) fn fail_page(&mut self, page: Page) {
        self.pagination.fail_page(page);
    }

    /// 是否尚未收齐专辑；加载状态不影响此判断。
    pub(crate) fn has_more(&self) -> bool {
        !self.pagination.exhausted()
    }

    /// 是否已登记续页请求且尚未消费成功或失败回包。
    pub(crate) fn is_loading(&self) -> bool {
        self.pagination.is_loading()
    }
}

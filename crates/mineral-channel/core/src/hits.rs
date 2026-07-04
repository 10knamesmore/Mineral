//! 搜索命中的一页结果 + 显式翻页信号。

/// 一页搜索命中 + 该源对「还有没有下一页」的显式表态。
///
/// `has_more` 是给上层榨干判定的**显式信号**:分页模型是页码型、或每页条数由服务端
/// 决定(与请求的 `limit` 无关)的源,靠「返回条数 < limit」推断会误判榨干——这类源
/// 应从响应的分页元信息(总页数 / 总条数)算出明确的 `Some`。
#[derive(Debug, Clone)]
pub struct SearchHits<T> {
    /// 本页命中项。
    pub items: Vec<T>,

    /// 是否还有下一页:`Some(true/false)` = 源明确知道;`None` = 源不知道,
    /// 上层回退「返回条数 < 请求 limit 即榨干」的推断。
    pub has_more: Option<bool>,
}

impl<T> SearchHits<T> {
    /// 带显式翻页信号构造一页命中。
    ///
    /// # Params:
    ///   - `items`: 本页命中项
    ///   - `has_more`: 是否还有下一页(源侧确知)
    pub fn new(items: Vec<T>, has_more: bool) -> Self {
        Self {
            items,
            has_more: Some(has_more),
        }
    }
}

/// 无翻页元信息的源直接把命中列表升格成一页(`has_more = None`,上层按条数推断)。
impl<T> From<Vec<T>> for SearchHits<T> {
    fn from(items: Vec<T>) -> Self {
        Self {
            items,
            has_more: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SearchHits;

    /// `From<Vec>` 升格:has_more 落 None(上层回退条数推断);`new` 落 Some。
    #[test]
    fn from_vec_leaves_has_more_unknown() {
        let hits = SearchHits::from(vec![1, 2, 3]);
        assert_eq!(hits.items.len(), 3);
        assert_eq!(hits.has_more, None, "Vec 升格不臆造翻页信号");
        let hits = SearchHits::new(vec![1], /*has_more*/ true);
        assert_eq!(hits.has_more, Some(true));
    }
}

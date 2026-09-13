//! 列表查询的分页参数与单页结果。

use serde::{Deserialize, Serialize};

/// 列表查询的分页参数。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Page {
    /// 起始偏移(从 0 起)。
    pub offset: u32,

    /// 单页返回上限。
    pub limit: u32,
}

impl Page {
    /// 构造分页参数。
    pub const fn new(offset: u32, limit: u32) -> Self {
        Self { offset, limit }
    }
}

impl Default for Page {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 30,
        }
    }
}

/// 保存一页列表条目及来源提供的翻页信号。
///
/// 页码分页或服务端固定页大小的来源应从响应的总页数、总条数或翻页标记
/// 得出明确的 `has_more`，避免上层按请求 `limit` 将短页误判为末页。
#[derive(Debug, Clone)]
pub struct PageResult<T> {
    /// 本页条目。
    pub items: Vec<T>,

    /// 是否还有下一页；`Some(true/false)` 表示 channel 已作判断，`None` 时上层
    /// 按「返回条数小于请求 `limit` 即末页」推断。
    ///
    /// channel 按条数推断时应使用过滤前的条数，避免因丢弃不可用条目而误判末页。
    pub has_more: Option<bool>,
}

impl<T> PageResult<T> {
    /// 使用明确的翻页信号构造一页列表。
    ///
    /// # Params:
    ///   - `items`: 本页条目。
    ///   - `has_more`: 是否还有下一页。
    pub fn new(items: Vec<T>, has_more: bool) -> Self {
        Self {
            items,
            has_more: Some(has_more),
        }
    }
}

/// 将列表转为单页结果，翻页信号保留为 `None`，由上层按条数推断。
impl<T> From<Vec<T>> for PageResult<T> {
    fn from(items: Vec<T>) -> Self {
        Self {
            items,
            has_more: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PageResult;

    /// 列表转换保留未知翻页状态，显式构造保留来源的翻页判断。
    #[test]
    fn from_vec_leaves_has_more_unknown() {
        let result = PageResult::from(vec![1, 2, 3]);
        assert_eq!(result.items, vec![1, 2, 3]);
        assert_eq!(result.has_more, None, "Vec 转换不臆造翻页信号");
        for has_more in [false, true] {
            let result = PageResult::new(vec![1], has_more);
            assert_eq!(result.items, vec![1]);
            assert_eq!(result.has_more, Some(has_more));
        }
    }
}

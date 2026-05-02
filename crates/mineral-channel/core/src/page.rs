use serde::{Deserialize, Serialize};

/// 列表查询的分页参数。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

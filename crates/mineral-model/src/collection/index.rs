use serde::{Deserialize, Serialize};

/// Collection canonical snapshot 中的 0-based absolute coordinate。
///
/// 该值属于 AlbumTrack / PlaylistEntry relation，不是 release numbering、queue index、
/// view index 或 durable entry identity。跨进程序列化固定使用 `u64`，只在本进程索引边界
/// 显式转换成 `usize`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CollectionIndex(u64);

impl CollectionIndex {
    /// 构造 0-based collection index。
    ///
    /// # Params:
    ///   - `value`: canonical snapshot 中的 absolute coordinate
    ///
    /// # Return:
    ///   强类型 collection index。
    #[inline]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 读取底层 fixed-width 0-based 数字。
    ///
    /// # Return:
    ///   `u64` coordinate。
    #[inline]
    pub const fn get(self) -> u64 {
        self.0
    }
}

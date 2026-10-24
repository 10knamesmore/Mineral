//! 状态修订的判等、排序与推进。

use std::num::TryFromIntError;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// 同一状态的修订号；只在同一条修订序列内比较。
///
/// 状态拥有者通过 `next()` 推进，消费者保存副本以判断变化。
/// 默认值是有效的初始修订；尚未观察到状态用 `Option<Revision>` 表示。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    /// 返回当前修订的下一版。
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// 从整数存储列恢复修订；负值无法表示有效修订。
impl TryFrom<i64> for Revision {
    type Error = TryFromIntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value).map(Self)
    }
}

/// 写入有符号整数存储列；超出列范围时返回转换错误。
impl TryFrom<Revision> for i64 {
    type Error = TryFromIntError;

    fn try_from(value: Revision) -> Result<Self, Self::Error> {
        Self::try_from(value.0)
    }
}

/// 跨线程推进的修订序列；读取与推进直接使用原子操作。
#[derive(Debug, Default)]
pub struct AtomicRevision(AtomicU64);

impl AtomicRevision {
    /// 读取当前修订。
    #[must_use]
    pub fn load(&self, ordering: Ordering) -> Revision {
        Revision(self.0.load(ordering))
    }

    /// 原子推进并返回新修订。
    pub fn next(&self, ordering: Ordering) -> Revision {
        Revision(self.0.fetch_add(1, ordering)).next()
    }
}

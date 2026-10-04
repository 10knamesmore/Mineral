//! 组件实例的进程内编号；帧记录只复制编号，不持有组件或共享分配。

use std::sync::atomic::{AtomicU64, Ordering};

/// 跨帧识别同一组件实例；移动保留编号，新组件重新发号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ComponentId(u64);

impl ComponentId {
    /// 为新组件发号；`Relaxed` 只保证发号唯一性，不承担组件数据的跨线程发布。
    pub(super) fn new() -> Self {
        static NEXT_COMPONENT_ID: AtomicU64 = AtomicU64::new(0);

        Self(NEXT_COMPONENT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

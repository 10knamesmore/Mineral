//! wire 类型 → 埋点类型的边缘适配(protocol 不依赖 stats,转换只在此处发生)。

use mineral_protocol::{QueueContextWire, QueueOp};

/// wire 编辑操作 → 埋点判别。
pub(super) fn stats_queue_op(op: &QueueOp) -> mineral_stats::QueueOp {
    match op {
        QueueOp::Remove(..) => mineral_stats::QueueOp::Remove,
        QueueOp::Move { .. } => mineral_stats::QueueOp::Move,
        QueueOp::ClearAbove(..) => mineral_stats::QueueOp::ClearAbove,
        QueueOp::ClearBelow(..) => mineral_stats::QueueOp::ClearBelow,
        QueueOp::ApplyTransform { .. } => mineral_stats::QueueOp::Transform,
        QueueOp::Undo => mineral_stats::QueueOp::Undo,
    }
}

/// 编辑作用到的那首歌;整表级操作(变换 / 撤销)无单曲归属。
pub(super) fn edited_song_id(op: &QueueOp) -> Option<mineral_model::SongId> {
    match op {
        QueueOp::Remove(at) | QueueOp::ClearAbove(at) | QueueOp::ClearBelow(at) => {
            Some(at.song_id.clone())
        }
        QueueOp::Move { at, .. } => Some(at.song_id.clone()),
        QueueOp::ApplyTransform { .. } | QueueOp::Undo => None,
    }
}

/// wire 队列语境 → 埋点 [`mineral_stats::QueueContext`]。
pub(super) fn queue_context_from_wire(wire: QueueContextWire) -> mineral_stats::QueueContext {
    use mineral_stats::QueueContext;
    match wire {
        // wire 侧总带原文;落库前由 recorder 按 search_queries 隐私档 redact。
        QueueContextWire::Search { query } => QueueContext::Search { query: Some(query) },
        QueueContextWire::Playlist { id, name } => QueueContext::Playlist { id, name },
        QueueContextWire::Album { id, name } => QueueContext::Album { id, name },
        QueueContextWire::Artist { id, name } => QueueContext::Artist { id, name },
        QueueContextWire::Manual => QueueContext::Manual,
        QueueContextWire::Unknown => QueueContext::Unknown,
    }
}

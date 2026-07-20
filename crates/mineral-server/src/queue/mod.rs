//! 队列:按 [`mineral_protocol::PlayMode`] 的导航推进,以及结构编辑(删除 / 重排 / 撤销)。

mod edit;
mod nav;

pub(crate) use edit::{apply, apply_order};
pub(crate) use nav::{
    QUEUE_CAP, advance_next, advance_prev, append, apply_play_mode, insert_next, next_in_queue,
    next_index,
};
// shuffle 边界与 prev 预测只被 apply_play_mode / advance_prev 内部调用,导出仅供测试直接驱动。
#[cfg(test)]
pub(crate) use nav::{enter_shuffle, exit_shuffle, prev_index};

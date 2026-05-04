//! Lane:按任务种类切分的执行域。

use serde::{Deserialize, Serialize};

/// 一组共享 worker 池 / 队列 / 并发预算的任务集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Lane {
    /// 按 channel 分 worker 池,跑 [`crate::ChannelFetchKind`] 系任务(my_playlists、
    /// songs_in_playlist 等)。
    ChannelFetch,

    /// 单一 worker 池,跑 [`crate::TaskKind::CoverArt`](裸 HTTP fetch + 图片解码)。
    CoverArt,
}

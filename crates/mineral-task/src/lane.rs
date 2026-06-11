//! Lane:按任务种类切分的执行域。

use serde::{Deserialize, Serialize};

/// 一组共享 worker 池 / 队列 / 并发预算的任务集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Lane {
    /// 按 channel 分 worker 池,跑 [`crate::ChannelFetchKind`] 系任务(my_playlists、
    /// songs_in_playlist 等)。
    ChannelFetch,

    /// 歌单写操作:**per-source 单 worker 串行**——同源两个写乱序到达远端
    /// 会丢更新,串行是顺序保证,不是性能取舍。
    PlaylistWrite,
}

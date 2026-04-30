//! Lane:按任务种类切分的执行域。

/// 一组共享 worker 池 / 队列 / 并发预算的任务集合。
///
/// 当前只实装 [`Lane::ChannelFetch`];其他 lane 是占位,等到接入对应任务种类时再实装。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    /// 按 channel 分 worker 池,跑 [`crate::ChannelFetchKind`] 系任务(my_playlists、
    /// songs_in_playlist 等)。
    ChannelFetch,
}

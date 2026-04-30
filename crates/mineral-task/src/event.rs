//! 任务推到 client 的事件载荷。

use mineral_model::{Playlist, PlaylistId, Song, SourceKind};

/// 任务完成时,channel 中央事件 buffer 推给 client 消费的载荷。
///
/// 失败任务不发 event(只在 [`crate::TaskHandle::done`] 上拿到 [`crate::TaskOutcome::Failed`]),
/// 详细错误进 mineral-log。
#[derive(Debug, Clone)]
pub enum TaskEvent {
    /// `MyPlaylists` 任务成功:某 channel 当前用户的歌单列表已到。
    PlaylistsFetched {
        /// 来源 channel。
        source: SourceKind,

        /// 拉到的歌单。
        playlists: Vec<Playlist>,
    },

    /// `PlaylistTracks` 任务成功:某歌单内的曲目已到。
    PlaylistTracksFetched {
        /// 歌单 id。
        id: PlaylistId,

        /// 曲目。
        tracks: Vec<Song>,
    },
}

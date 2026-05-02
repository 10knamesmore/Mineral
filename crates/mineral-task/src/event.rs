//! 任务推到 client 的事件载荷。

use std::collections::HashSet;
use std::sync::Arc;

use image::DynamicImage;
use mineral_model::{Lyrics, MediaUrl, PlayUrl, Playlist, PlaylistId, Song, SongId, SourceKind};

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

    /// `LikedSongIds` 任务成功:某 channel 当前用户喜欢(♥)的歌曲 ID 集合已到。
    LikedSongIdsFetched {
        /// 来源 channel。
        source: SourceKind,

        /// 喜欢的歌曲 ID 集合。
        ids: HashSet<SongId>,
    },

    /// `PlaylistTracks` 任务成功:某歌单内的曲目已到。
    PlaylistTracksFetched {
        /// 歌单 id。
        id: PlaylistId,

        /// 曲目。
        tracks: Vec<Song>,
    },

    /// `SongUrl` 任务成功:可播放 URL 解析就绪。
    PlayUrlReady {
        /// 关联的歌曲 id。
        song_id: SongId,

        /// 解析出的播放 URL + 元信息。
        play_url: PlayUrl,
    },

    /// `Lyrics` 任务成功:歌词数据就绪。
    LyricsReady {
        /// 关联的歌曲 id。
        song_id: SongId,

        /// 各格式歌词(LRC / yrc / 翻译 / 罗马音)。
        lyrics: Lyrics,
    },

    /// `CoverArt` 任务成功:封面图已 fetch + decode。
    CoverReady {
        /// 图片 URL,UI 端按它进 cover_cache。
        url: MediaUrl,

        /// 解码后的 RGB 图。`Arc` 让 cache 命中和 render 共享同一份像素。
        image: Arc<DynamicImage>,
    },
}

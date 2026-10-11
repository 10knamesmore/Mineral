//! 曲库项目的显示模型，以及按歌曲身份补充的喜欢和播放次数。

use mineral_model::{Album, AlbumTrack, CollectionIndex, Playlist, PlaylistEntry, Song};

/// 一条歌单 + UI 装饰。
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistView {
    /// 底层 model。
    pub data: Playlist,
}

/// 收藏专辑的元信息；详情曲目由曲库缓存按专辑身份保存。
#[derive(Clone, Debug, PartialEq)]
pub struct AlbumView {
    /// 音乐来源映射后的统一专辑模型。
    pub data: Album,
}

/// 列表绘制需要的曲目关系投影，所属专辑或歌单由外层身份区分。
#[derive(Clone, Debug, PartialEq)]
pub struct CollectionTrackData {
    /// 来源给出的集合内绝对位置，保留重复曲目及缺失位置。
    pub index: CollectionIndex,

    /// 此位置对应的歌曲。
    pub song: Song,
}

impl From<PlaylistEntry> for CollectionTrackData {
    fn from(entry: PlaylistEntry) -> Self {
        Self {
            index: entry.index,
            song: entry.song,
        }
    }
}

impl From<AlbumTrack> for CollectionTrackData {
    fn from(track: AlbumTrack) -> Self {
        Self {
            index: track.index,
            song: track.song,
        }
    }
}

/// 一条集合内曲目及其用户数据装饰。
///
/// 保留来源的 CollectionIndex；歌单或专辑身份由所属列表提供。
#[derive(Clone, Debug, PartialEq)]
pub struct CollectionTrackView {
    /// 曲目与来源给定的集合内位置。
    pub data: CollectionTrackData,

    /// 该 relation 指向的 Song 是否已收藏。
    pub loved: bool,

    /// 该 relation 指向的 Song 在 Mineral 中自然播完的次数；未采集时为 `None`。
    pub plays: Option<u32>,
}

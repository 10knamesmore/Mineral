//! 跨 channel 的统一数据模型。
//!
//! 所有 channel 的输出最终是平铺合并的——除 `source` 标记外没有 channel-specific
//! 字段,上层 UI 拿到 `Vec<Song>` 不需要知道哪首来自哪。

pub mod album;
pub mod artist;
pub mod bitrate;
pub mod ids;
pub mod lyrics;
pub mod play_url;
pub mod playlist;
pub mod refs;
pub mod search;
pub mod song;
pub mod source;
pub mod url;

pub use album::Album;
pub use artist::Artist;
pub use bitrate::BitRate;
pub use ids::{AlbumId, ArtistId, PlaylistId, SongId, UserId};
pub use lyrics::Lyrics;
pub use play_url::PlayUrl;
pub use playlist::Playlist;
pub use refs::{AlbumRef, ArtistRef};
pub use search::SearchKind;
pub use song::Song;
pub use source::SourceKind;
pub use url::MediaUrl;

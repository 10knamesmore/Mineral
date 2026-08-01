//! Album / Playlist 中 Song membership 的显式 relation。

/// Album membership relation。
mod album_track;
/// Collection canonical coordinate。
mod index;
/// Playlist membership relation。
mod playlist_entry;

pub use album_track::AlbumTrack;
pub use index::CollectionIndex;
pub use playlist_entry::PlaylistEntry;

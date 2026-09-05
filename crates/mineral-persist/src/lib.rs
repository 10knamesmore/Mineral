//! Mineral 本地持久化层:server / client 各自的 sqlite 库门面 + 通用文件缓存索引原语。
//!
//! - [`ServerStore`]:daemon 的库(`mineral.db`)——功能状态(歌元数据 / 收藏 / 评分 / 歌单缓存 /
//!   会话)+ 它名下的音频缓存索引表(`audio_cache`)。下载导出由文件系统目录直接承载,
//!   不在持久层建索引表。
//! - [`ClientStore`]:TUI 客户端的库(`tui.db`)——封面缓存索引(`cover_cache`)、
//!   UI 偏好(`ui_prefs`)与歌单内光标位置记忆(`track_pos`)。
//! - [`CacheIndex`]:表级原语(内存镜像 sync 读 + 写穿透),由上面两个库门面取得,二者共用。

mod cache_index;
mod client_store;
mod db;
mod entity;
mod migration;
mod pool;
mod server_store;

pub use cache_index::{CacheEntryStat, CacheIndex, CacheStats, Evicted};
pub use client_store::{ClientStore, TrackPosRow};
pub use db::{
    CachedPlaylistEntry, NamespaceStore, PlaylistCacheEntry, RESERVED_KEYS, SessionSnapshot,
    SessionStore,
};
pub use server_store::{PlaylistCacheStats, ServerStore};

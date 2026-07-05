//! Mineral 本地持久化层:server / client 各自的 sqlite 库门面 + 通用文件缓存索引原语。
//!
//! - [`ServerStore`]:daemon 的库(`mineral.db`)——结构态(歌元数据 / 统计 / 历史 / 歌单缓存 /
//!   会话)+ 它名下的音频缓存索引表(`audio_cache`)。下载导出**不走索引**:导出目录本身即真相
//!   (见 server 的播放解析),持久层不再为它存表。
//! - [`ClientStore`]:TUI 客户端的库(`tui.db`)——封面缓存索引(`cover_cache`)、
//!   UI 偏好(`ui_prefs`)与歌单内光标位置记忆(`track_pos`)。
//! - [`CacheIndex`]:表级原语(内存镜像 sync 读 + 写穿透),由上面两个库门面取得,二者共用。

mod cache_index;
mod client_store;
mod db;
mod pool;
mod server_store;

pub use cache_index::{CacheEntryStat, CacheIndex, CacheStats};
pub use client_store::{ClientStore, TrackPosRow};
pub use db::{
    HistoryEntry, NamespaceStore, PlaylistCacheEntry, RESERVED_KEYS, SessionSnapshot, SessionStore,
    SongStats,
};
pub use server_store::{PlaylistCacheStats, ServerStore};

//! Mineral 本地持久化层:server / client 各自的 sqlite 库门面 + 通用文件缓存索引原语。
//!
//! - [`ServerStore`]:daemon 的库(`mineral.db`)——结构态(歌元数据 / 统计 / 历史 / 歌单缓存 /
//!   会话)+ 它名下的缓存索引表(`audio_cache` / `download_export`)。
//! - [`ClientStore`]:TUI 客户端的库(`tui.db`)——目前住封面缓存索引(`cover_cache`)。
//! - [`CacheIndex`]:表级原语(内存镜像 sync 读 + 写穿透),由上面两个库门面取得,二者共用。

mod cache_index;
mod client_store;
mod db;
mod pool;
mod server_store;

pub use cache_index::CacheIndex;
pub use client_store::ClientStore;
pub use db::{
    HistoryEntry, NamespaceStore, PlaylistCacheEntry, SessionSnapshot, SessionStore, SongStats,
};
pub use server_store::ServerStore;

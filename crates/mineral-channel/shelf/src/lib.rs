//! shelf source 的 channel 实现。
//!
//! shelf 是「用户自管的、经 storage backend(MVP 为文件系统)可达的音频收藏」接入
//! [`mineral_channel_core::MusicChannel`] 的连接器。组织语义由用户经配置声明;索引落
//! persist、扫描是 daemon 级任务。
//!
//! 本 crate 对外暴露 [`ShelfStorage`] backend 抽象(供未来远端 backend 实现)与 fs 实现。

mod channel;
mod index;
mod scan;
mod storage;

pub use channel::ShelfChannel;
pub use index::{reconcile, scan_and_index};
pub use scan::{ScanOptions, ScannedDir, ScannedFile, scan};
pub use storage::{Entry, EntryKind, FsStorage, ShelfReader, ShelfStorage};

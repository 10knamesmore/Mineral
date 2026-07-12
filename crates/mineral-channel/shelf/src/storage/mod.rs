//! shelf 的 storage backend 抽象与实现。
//!
//! 扫描 / 探测 / 取播放目标一律经 [`ShelfStorage`],上层(索引 / organize / 扫描生命周期)
//! 全部 backend 无关:换后端(fs → WebDAV/SFTP/S3)只加实现、不改上层。扫描器**不许**
//! 绕过 trait 直触 `std::fs`,否则未来抽取成本随调用散开而暴涨。

mod backend;
mod fs;

pub use backend::{Entry, EntryKind, ShelfReader, ShelfStorage};
pub use fs::FsStorage;

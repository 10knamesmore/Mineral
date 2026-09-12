//! DB-backed 文件缓存索引:内存镜像(sync 读)+ SQLite 写穿透(async 写)。
//!
//! 缓存文件以 `<root>/<subdir>/<file_name>` 落盘,身份到文件的映射存进 SQLite 的
//! `(key, relpath, bytes, last_access)` 表。每次写入当场 `UPSERT`,不依赖 Drop flush。
//!
//! 读路径全 **sync**(内存镜像):供 `resolve_local` 在 sync 播放解析里命中本地副本。命中只是
//! 优化、永不是正确性依赖——任何漂移(文件缺失)一律当 miss,内存删项自愈;DB 里的死记录由下次
//! `open` 的启动对账清掉。`last_access`(LRU 提示)与漂移删除**只改内存、不落库**:重启后 LRU
//! 退化成近似,可接受。真正 durable 的写(入库 / 驱逐)走 async 写穿透。

mod file_cache;
mod file_placement;
mod storage;

#[cfg(test)]
mod tests;

pub use file_cache::{CacheEntryStat, CacheIndex, CacheStats, Evicted};
pub(crate) use storage::CacheTable;

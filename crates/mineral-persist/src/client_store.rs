//! ClientStore 句柄:客户端(TUI)自己的 sqlite 库(`tui.db`),与 server 的
//! [`ServerStore`](crate::ServerStore) 并列、各占一个库文件。
//!
//! 当前只住封面缓存索引(`cover_cache` 表);后续客户端态(UI 偏好等)也归这里。
//! 文件本体落 [`mineral_paths::cover_cache_dir`] 等可清理目录,索引落 `tui.db`。

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::CacheIndex;

/// 客户端持久化句柄。持有 `tui.db` 连接池,按需打开其中的缓存索引表。
pub struct ClientStore {
    /// `tui.db` 连接池。
    pool: SqlitePool,
}

impl ClientStore {
    /// 打开(或创建)客户端库文件。
    ///
    /// # Params:
    ///   - `db_path`: `tui.db` 路径(父目录需已存在;不存在则建文件)
    ///
    /// # Return:
    ///   就绪句柄;连接失败返回 `Err`(调用方应降级,如封面不缓存)。
    pub async fn open(db_path: &Path) -> color_eyre::Result<Self> {
        let pool = crate::pool::connect(db_path).await?;
        Ok(Self { pool })
    }

    /// 封面缓存索引(`cover_cache` 表,LRU 驱逐)。键 = 封面 URL。
    ///
    /// # Params:
    ///   - `root`: 封面文件根目录(`relpath` 相对它)
    ///   - `capacity`: 容量上限字节(LRU 满了驱逐最旧)
    ///
    /// # Return:
    ///   就绪索引;建表 / 载入失败返回 `Err`。
    pub async fn cover_cache(
        &self,
        root: PathBuf,
        capacity: u64,
    ) -> color_eyre::Result<CacheIndex> {
        CacheIndex::open(self.pool.clone(), "cover_cache", root, Some(capacity)).await
    }
}

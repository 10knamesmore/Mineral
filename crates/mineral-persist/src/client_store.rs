//! ClientStore 句柄:客户端(TUI)自己的 sqlite 库(`tui.db`),与 server 的
//! [`ServerStore`](crate::ServerStore) 并列、各占一个库文件。
//!
//! 住封面缓存索引(`cover_cache` 表)与 UI 偏好(`ui_prefs` 表,通用 KV)。
//! 文件本体落 [`mineral_paths::cover_cache_dir`] 等可清理目录,索引落 `tui.db`。

use std::path::{Path, PathBuf};

use color_eyre::eyre::WrapErr;
use sqlx::SqlitePool;

use crate::CacheIndex;

/// 客户端持久化句柄。持有 `tui.db` 连接池,按需打开其中的缓存索引表 / 读写 UI 偏好。
pub struct ClientStore {
    /// `tui.db` 连接池。
    pool: SqlitePool,
}

impl ClientStore {
    /// 打开(或创建)客户端库文件,并就绪 `ui_prefs` 表。
    ///
    /// # Params:
    ///   - `db_path`: `tui.db` 路径(父目录需已存在;不存在则建文件)
    ///
    /// # Return:
    ///   就绪句柄;连接 / 建表失败返回 `Err`(调用方应降级,如封面不缓存、偏好不存)。
    pub async fn open(db_path: &Path) -> color_eyre::Result<Self> {
        let pool = crate::pool::connect(db_path).await?;
        Self::with_pool(pool).await
    }

    /// 用现成连接池组装句柄并就绪 `ui_prefs` 表(测试用内存池入口)。
    async fn with_pool(pool: SqlitePool) -> color_eyre::Result<Self> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS ui_prefs (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .wrap_err("建 ui_prefs 表失败")?;
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

    /// 读一条 UI 偏好(`ui_prefs` 表)。
    ///
    /// # Params:
    ///   - `key`: 偏好键(如 `"lyric_extra"`)
    ///
    /// # Return:
    ///   键存在为 `Some(值)`,不存在为 `None`。
    pub async fn get_pref(&self, key: &str) -> color_eyre::Result<Option<String>> {
        sqlx::query_scalar::<_, String>("SELECT value FROM ui_prefs WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .wrap_err_with(|| format!("读 ui_prefs 失败 key={key}"))
    }

    /// 写一条 UI 偏好(单条 upsert,同键覆盖)。
    ///
    /// # Params:
    ///   - `key`: 偏好键
    ///   - `value`: 偏好值(调用方自行定义稳定字符串编码)
    pub async fn set_pref(&self, key: &str, value: &str) -> color_eyre::Result<()> {
        sqlx::query(
            "INSERT INTO ui_prefs (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .wrap_err_with(|| format!("写 ui_prefs 失败 key={key}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::ClientStore;

    /// 开一个内存 sqlite 的 [`ClientStore`](每个测试独立)。
    async fn mem_store() -> color_eyre::Result<ClientStore> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        ClientStore::with_pool(pool).await
    }

    /// ui_prefs round-trip:set 后 get 读回原值;同键再 set 覆盖;未写键为 `None`。
    #[tokio::test]
    async fn ui_prefs_round_trip_and_overwrite() -> color_eyre::Result<()> {
        let store = mem_store().await?;
        assert_eq!(store.get_pref("lyric_extra").await?, None, "未写键应 None");
        store.set_pref("lyric_extra", "translation").await?;
        assert_eq!(
            store.get_pref("lyric_extra").await?.as_deref(),
            Some("translation")
        );
        store.set_pref("lyric_extra", "none").await?;
        assert_eq!(
            store.get_pref("lyric_extra").await?.as_deref(),
            Some("none"),
            "同键 upsert 应覆盖"
        );
        Ok(())
    }

    /// 不同键互不串扰。
    #[tokio::test]
    async fn ui_prefs_keys_are_independent() -> color_eyre::Result<()> {
        let store = mem_store().await?;
        store.set_pref("a", "1").await?;
        store.set_pref("b", "2").await?;
        assert_eq!(store.get_pref("a").await?.as_deref(), Some("1"));
        assert_eq!(store.get_pref("b").await?.as_deref(), Some("2"));
        Ok(())
    }
}

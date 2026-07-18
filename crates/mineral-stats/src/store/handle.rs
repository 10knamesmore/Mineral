//! `StatsStore` 句柄:stats.db 的打开 / 迁移 / 降级空实现。
//!
//! 降级(库打不开)走 null-object:所有写静默 no-op、所有查询返回空,埋点失效但
//! 播放照常。writer 另有 [`StatsStore::enabled`] 可在源头短路。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::WrapErr as _;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// 编译期嵌入的迁移链(用户机器上无需随附 SQL 文件)。
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// stats.db 门面。Clone 廉价(内部 `Arc`)。
#[derive(Clone)]
pub struct StatsStore {
    /// 内部后端:真实 sqlite 或降级 null。
    backend: Arc<Backend>,
}

/// 内部后端。
enum Backend {
    /// 真实 sqlite 连接池。
    Sqlite(SqlitePool),

    /// 降级 no-op(库打不开时)。
    Disabled,
}

impl StatsStore {
    /// 打开(或创建)stats.db 并跑迁移。
    ///
    /// 显式设 WAL + NORMAL 同步 + 外键 + busy_timeout:这些是承重语义(CLI 与 daemon
    /// 在 WAL 下并发读写、plays→sessions 外键约束),不吃库隐式默认。父目录需调用方先
    /// 建(sqlite `create_if_missing` 只建文件不建目录)。
    ///
    /// # Params:
    ///   - `db_path`: stats.db 完整路径
    ///
    /// # Return:
    ///   打开成功的句柄;打开 / 迁移失败冒泡(调用方决定是否降级)
    pub async fn open(db_path: &Path) -> color_eyre::Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .wrap_err_with(|| format!("打开 stats.db 失败 path={}", db_path.display()))?;
        MIGRATOR.run(&pool).await.wrap_err("stats.db 迁移失败")?;
        Ok(Self {
            backend: Arc::new(Backend::Sqlite(pool)),
        })
    }

    /// 降级 no-op 句柄(库打不开时用,埋点静默失效、播放照常)。
    pub fn disabled() -> Self {
        Self {
            backend: Arc::new(Backend::Disabled),
        }
    }

    /// 是否启用(非降级)。writer 据此在源头短路,免组装后续记录命令。
    pub fn enabled(&self) -> bool {
        matches!(self.backend.as_ref(), Backend::Sqlite(_))
    }

    /// 取内部连接池;降级时 `None`。写 / 查方法据此 `let-else` 早返回中性值。
    pub(crate) fn pool(&self) -> Option<&SqlitePool> {
        match self.backend.as_ref() {
            Backend::Sqlite(pool) => Some(pool),
            Backend::Disabled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StatsStore;
    use sqlx::SqlitePool;

    /// 建落盘临时库(端到端过真实迁移),返回 `TempDir`(存活即目录存活)与句柄。
    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    /// 从句柄取 live pool,降级则测试失败(不 unwrap)。
    fn live(store: &StatsStore) -> color_eyre::Result<&SqlitePool> {
        store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("期望 live pool,得到 disabled"))
    }

    #[tokio::test]
    async fn open_sets_wal_and_foreign_keys() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let pool = live(&store)?;
        let mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
            .fetch_one(pool)
            .await?;
        assert_eq!(mode.to_lowercase(), "wal", "WAL 是承重设计,须断言");
        let fk = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
            .fetch_one(pool)
            .await?;
        assert_eq!(fk, 1, "外键须开启(plays→sessions)");
        Ok(())
    }

    #[tokio::test]
    async fn migrations_create_all_registered_tables() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let pool = live(&store)?;
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations'",
        )
        .fetch_one(pool)
        .await?;
        // 表数从事件表注册表派生(加表只改 EVENT_TABLES 一处,漏挂由
        // prune_registry_matches_migration 兜):core 本体 plays + sessions + songs 维表。
        let want = i64::try_from(crate::store::prune::EVENT_TABLES.len() + 3)?;
        assert_eq!(count, want, "plays + sessions + songs 维表 + 全部事件表");
        Ok(())
    }

    #[tokio::test]
    async fn disabled_has_no_pool() {
        let store = StatsStore::disabled();
        assert!(!store.enabled());
        assert!(store.pool().is_none());
    }
}

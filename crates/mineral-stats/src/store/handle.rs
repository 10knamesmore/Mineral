//! `StatsStore` 句柄:stats.db 的打开 / 迁移 / 降级空实现。
//!
//! 降级(库打不开)走 null-object:所有写静默 no-op、所有查询返回空,埋点失效但
//! 播放照常。writer 另有 [`StatsStore::enabled`] 可在源头短路。

use std::path::Path;
use std::sync::Arc;

use color_eyre::eyre::WrapErr as _;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::migration::Migrator;

/// stats.db 门面。Clone 廉价(内部 `Arc`)。
#[derive(Clone)]
pub struct StatsStore {
    /// 内部后端:真实 sqlite 或降级 null。
    backend: Arc<Backend>,
}

/// 内部后端。
enum Backend {
    /// 真实 sqlite 连接池。
    Sqlite(DatabaseConnection),

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
        let mut options = ConnectOptions::new(format!("sqlite://{}?mode=rwc", db_path.display()));
        // 单连接常驻，使初始化的连接级 PRAGMA 在句柄生命周期内保持生效。
        options
            .max_connections(/*value*/ 1)
            .min_connections(/*value*/ 1)
            .idle_timeout(/*value*/ None)
            .max_lifetime(/*lifetime*/ None)
            .after_connect(|connection| {
                Box::pin(async move {
                    connection
                        .execute_unprepared(
                            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; \
                     PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
                        )
                        .await?;
                    Ok(())
                })
            });
        let pool = Database::connect(options)
            .await
            .wrap_err_with(|| format!("打开 stats.db 失败 path={}", db_path.display()))?;
        Migrator::up(&pool, /*steps*/ None)
            .await
            .wrap_err("stats.db 迁移失败")?;
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
    pub(crate) fn pool(&self) -> Option<&DatabaseConnection> {
        match self.backend.as_ref() {
            Backend::Sqlite(pool) => Some(pool),
            Backend::Disabled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StatsStore;
    use crate::migration::{Migrator, test_support::table_names};
    use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;

    /// 建落盘临时库(端到端过真实迁移),返回 `TempDir`(存活即目录存活)与句柄。
    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    /// 从句柄取 live pool,降级则测试失败(不 unwrap)。
    fn live(store: &StatsStore) -> color_eyre::Result<&DatabaseConnection> {
        store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("期望 live pool,得到 disabled"))
    }

    #[tokio::test]
    async fn open_sets_wal_and_foreign_keys() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let db = live(&store)?;
        let mode = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA journal_mode",
            ))
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("missing journal mode"))?
            .try_get_by_index::<String>(/*index*/ 0)?;
        assert_eq!(mode.to_lowercase(), "wal", "WAL 是承重设计,须断言");
        let fk = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA foreign_keys",
            ))
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("missing foreign_keys"))?
            .try_get_by_index::<i64>(/*index*/ 0)?;
        assert_eq!(fk, 1, "外键须开启(plays→sessions)");
        Ok(())
    }

    #[tokio::test]
    async fn migrations_create_all_registered_tables() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let count = table_names(live(&store)?).await?.len();
        let want = crate::store::prune::EVENT_TABLES.len() + 4;
        assert_eq!(
            count, want,
            "plays + sessions + songs/song_artists 维表 + 全部事件表"
        );
        Ok(())
    }

    /// 完整回滚移除全部业务表，重新执行 up 可恢复完整结构。
    #[tokio::test]
    async fn migration_up_down_up() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let db = live(&store)?;
        let mut before = table_names(db).await?;
        before.sort();
        Migrator::down(db, /*steps*/ None).await?;
        assert!(table_names(db).await?.is_empty());
        Migrator::up(db, /*steps*/ None).await?;
        let mut after = table_names(db).await?;
        after.sort();
        assert_eq!(before, after, "up/down/up 后应恢复全部业务表");
        Ok(())
    }

    #[tokio::test]
    async fn disabled_has_no_pool() {
        let store = StatsStore::disabled();
        assert!(!store.enabled());
        assert!(store.pool().is_none());
    }
}

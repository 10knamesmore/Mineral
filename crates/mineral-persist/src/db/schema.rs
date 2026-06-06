//! 结构态 schema 与运行时建表。

use color_eyre::eyre::WrapErr;
use mineral_log::debug;
use sqlx::SqlitePool;

/// 全部 CREATE TABLE IF NOT EXISTS 语句(规范化、无 JSON 列),见同目录 `schema.sql`。
const SCHEMA: &str = include_str!("schema.sql");

/// 在连接池上建好全部表(幂等)。
///
/// # Params:
///   - `pool`: 已打开的 sqlite 连接池
///
/// # Return:
///   建表成功返回 `Ok(())`。
pub(crate) async fn ensure_schema(pool: &SqlitePool) -> color_eyre::Result<()> {
    sqlx::raw_sql(SCHEMA)
        .execute(pool)
        .await
        .wrap_err("建表(ensure_schema)失败")?;
    add_column_best_effort(pool, "song_stats", "rating INTEGER").await?;
    debug!(target: "persist", "建表完成");
    Ok(())
}

/// 老库升级:尽力 `ALTER TABLE .. ADD COLUMN ..`。
///
/// `CREATE TABLE IF NOT EXISTS` 不会改已有表,新增列必须走 ALTER;而
/// `ADD COLUMN` 非幂等(列已存在报 `duplicate column name`),这里按错误信息
/// 判别后吞掉该类错误,其余照常冒泡。
///
/// # Params:
///   - `table`: 目标表名
///   - `column_def`: 列定义(如 `"rating INTEGER"`)
async fn add_column_best_effort(
    pool: &SqlitePool,
    table: &str,
    column_def: &str,
) -> color_eyre::Result<()> {
    let sql = format!("ALTER TABLE {table} ADD COLUMN {column_def}");
    match sqlx::raw_sql(&sql).execute(pool).await {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("duplicate column name") => Ok(()),
        Err(e) => Err(e).wrap_err_with(|| format!("升级表结构失败:{sql}")),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::ensure_schema;

    /// 全量 ensure_schema 跑两次幂等(CREATE TABLE IF NOT EXISTS + 尽力 ALTER)。
    #[tokio::test]
    async fn ensure_schema_is_idempotent() -> color_eyre::Result<()> {
        let pool = SqlitePoolOptions::new().connect("sqlite::memory:").await?;
        ensure_schema(&pool).await?;
        ensure_schema(&pool).await?;
        Ok(())
    }

    /// 老库升级:已有无 rating 列的 song_stats 时,ensure_schema 经尽力 ALTER 补列
    /// (CREATE IF NOT EXISTS 不会改已有表),且再跑一次不因列已存在报错。
    #[tokio::test]
    async fn ensure_schema_adds_rating_to_legacy_song_stats() -> color_eyre::Result<()> {
        let pool = SqlitePoolOptions::new().connect("sqlite::memory:").await?;
        sqlx::raw_sql(
            "CREATE TABLE song_stats (\
                 namespace TEXT NOT NULL,\
                 song_value TEXT NOT NULL,\
                 play_count INTEGER NOT NULL DEFAULT 0,\
                 skip_count INTEGER NOT NULL DEFAULT 0,\
                 total_listen_ms INTEGER NOT NULL DEFAULT 0,\
                 last_played_at INTEGER, loved_at INTEGER,\
                 PRIMARY KEY (namespace, song_value));",
        )
        .execute(&pool)
        .await?;
        ensure_schema(&pool).await?;
        ensure_schema(&pool).await?;
        sqlx::query("SELECT rating FROM song_stats")
            .fetch_all(&pool)
            .await?;
        Ok(())
    }
}

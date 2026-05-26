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
    debug!(target: "persist", "建表完成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::ensure_schema;

    /// 全量 ensure_schema 跑两次幂等(CREATE TABLE IF NOT EXISTS)。
    #[tokio::test]
    async fn ensure_schema_is_idempotent() -> color_eyre::Result<()> {
        let pool = SqlitePoolOptions::new().connect("sqlite::memory:").await?;
        ensure_schema(&pool).await?;
        ensure_schema(&pool).await?;
        Ok(())
    }
}

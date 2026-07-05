//! 结构态 schema:版本化迁移(`migrations/` 编译期嵌入,启动时按序补跑)。
//!
//! 规矩:每次结构变更**新增**一个 `migrations/NNNN_*.sql`,**永不改已发布的迁移**——
//! 迁移器对已应用条目做 checksum 校验,改历史会让所有老库启动报 `VersionMismatch`。
//! 需要程序逻辑的数据修复不进迁移:结构由迁移管,数据由启动时的幂等修复步管
//! (靠数据自身状态判断是否还需要做,如 `WHERE new_col IS NULL`)。

use color_eyre::eyre::WrapErr;
use mineral_log::debug;
use sqlx::SqlitePool;

/// 全部版本化迁移,编译期嵌入二进制(用户机器上不需要 SQL 文件)。
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// 把库推进到最新 schema 版本(幂等:已应用的迁移经 `_sqlx_migrations` 记账跳过,
/// 新库从零建齐)。每条迁移与其记账在同一事务里,不产生「跑一半」的脏库。
///
/// # Params:
///   - `pool`: 已打开的 sqlite 连接池
///
/// # Return:
///   迁移到最新版本返回 `Ok(())`;建于迁移机制之前的老库(表已存在、无记账)会在
///   baseline 撞「表已存在」报错,错误指引用户重建。
pub(crate) async fn ensure_schema(pool: &SqlitePool) -> color_eyre::Result<()> {
    MIGRATOR.run(pool).await.wrap_err(
        "schema 迁移失败;若此库建于迁移机制引入之前,请停掉 daemon 后运行 \
         `mineral cache reset --yes` 删库重建(会丢失播放统计 / 喜欢 / 历史)",
    )?;
    debug!(target: "persist", "schema 迁移完成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::ensure_schema;

    /// 迁移幂等:跑两次第二次经记账全跳过,不因「表已存在」报错。
    #[tokio::test]
    async fn ensure_schema_is_idempotent() -> color_eyre::Result<()> {
        let pool = SqlitePoolOptions::new().connect("sqlite::memory:").await?;
        ensure_schema(&pool).await?;
        ensure_schema(&pool).await?;
        Ok(())
    }

    /// 约束由库执行,不靠代码纪律:song_kv 的「vtype 与值列相符」CHECK、
    /// session_state 的「当前曲成对可空」CHECK、playlist_tracks → playlist_cache 外键
    /// (sqlx 的 sqlite 默认 pragma 带 `foreign_keys=ON`)各自拒绝坏行。
    #[tokio::test]
    async fn schema_constraints_reject_bad_rows() -> color_eyre::Result<()> {
        let pool = SqlitePoolOptions::new().connect("sqlite::memory:").await?;
        ensure_schema(&pool).await?;

        // song_kv:vtype='int' 但 int_val 为 NULL → CHECK 拒绝。
        let kv = sqlx::query(
            "INSERT INTO song_kv(namespace,song_value,key,vtype,text_val) \
             VALUES ('netease','1','k','int','oops')",
        )
        .execute(&pool)
        .await;
        assert!(kv.is_err(), "vtype 与值列不符应被 CHECK 拒绝");

        // session_state:cur_namespace 有值而 cur_song_value 为 NULL(半空对)→ CHECK 拒绝。
        let half = sqlx::query(
            "INSERT INTO session_state(id,cur_namespace,cur_song_value,position_ms,play_mode,volume,updated_at) \
             VALUES (0,'netease',NULL,0,'sequential',1.0,0)",
        )
        .execute(&pool)
        .await;
        assert!(half.is_err(), "当前曲半空对应被 CHECK 拒绝");

        // playlist_tracks:引用不存在的 playlist_cache 行 → FK 拒绝。
        let orphan = sqlx::query(
            "INSERT INTO playlist_tracks(namespace,playlist_id,position,song_value) \
             VALUES ('netease','nope',0,'s1')",
        )
        .execute(&pool)
        .await;
        assert!(orphan.is_err(), "孤儿曲目行应被外键拒绝");
        Ok(())
    }

    /// 建于迁移机制之前的老库(表已存在、无迁移记账):baseline 裸建表撞错,
    /// 错误信息带 `mineral cache reset` 重建指引——刻意响亮失败,不静默收编旧结构。
    #[tokio::test]
    async fn pre_migration_db_fails_loud_with_reset_hint() -> color_eyre::Result<()> {
        let pool = SqlitePoolOptions::new().connect("sqlite::memory:").await?;
        sqlx::raw_sql(
            "CREATE TABLE song_meta (\
                 namespace TEXT NOT NULL,\
                 song_value TEXT NOT NULL,\
                 name TEXT NOT NULL,\
                 duration_ms INTEGER NOT NULL,\
                 PRIMARY KEY (namespace, song_value));",
        )
        .execute(&pool)
        .await?;
        let err = match ensure_schema(&pool).await {
            Ok(()) => return Err(color_eyre::eyre::eyre!("老库应报错而非静默通过")),
            Err(e) => e,
        };
        let chain = format!("{err:#}");
        assert!(
            chain.contains("mineral cache reset"),
            "错误应带重建指引,实际:{chain}"
        );
        Ok(())
    }
}

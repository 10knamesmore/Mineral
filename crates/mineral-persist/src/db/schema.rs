//! 服务状态数据库的结构初始化。

use mineral_log::debug;
use sea_orm::DatabaseConnection;
use sea_orm_migration::MigratorTrait;

use crate::migration::ServerMigrator;

/// 按迁移账本应用尚未执行的 Rust 结构变更，每一步在事务内完成。
///
/// # Params:
///   - `pool`: 已打开的 sqlite 连接池
///
/// # Return:
///   迁移到最新版本返回 `Ok(())`;建于迁移机制之前的老库(表已存在、无记账)会在
///   baseline 撞「表已存在」报出迁移错误。
pub(crate) async fn ensure_schema(pool: &DatabaseConnection) -> crate::Result<()> {
    ServerMigrator::up(pool, /*steps*/ None)
        .await
        .map_err(|source| crate::Error::Migration {
            database: "server",
            source,
        })?;
    debug!(target: "persist", "schema 迁移完成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_schema;
    use crate::entity::{playlist_entries, session_state, song_favorites, song_kv, song_meta};
    use crate::migration::{ClientMigrator, ServerMigrator};
    use sea_orm::sea_query::{ColumnDef, Table};
    use sea_orm::{
        ConnectOptions, ConnectionTrait, Database, DatabaseConnection, EntityTrait, PaginatorTrait,
        Set,
    };
    use sea_orm_migration::MigratorTrait;

    /// 每次验证使用独立的单连接内存数据库。
    async fn memory_database() -> color_eyre::Result<DatabaseConnection> {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(/*value*/ 1);
        Ok(Database::connect(options).await?)
    }

    /// 已应用的迁移重复运行时不重建表。
    #[tokio::test]
    async fn ensure_schema_is_idempotent() -> color_eyre::Result<()> {
        let db = memory_database().await?;
        ensure_schema(&db).await?;
        ensure_schema(&db).await?;
        Ok(())
    }

    /// 值类型、成对歌曲身份和歌单外键必须由数据库拒绝非法记录。
    #[tokio::test]
    async fn schema_constraints_reject_bad_rows() -> color_eyre::Result<()> {
        let db = memory_database().await?;
        ensure_schema(&db).await?;
        let kv = song_kv::Entity::insert(song_kv::ActiveModel {
            namespace: Set("netease".to_owned()),
            song_value: Set("1".to_owned()),
            key: Set("k".to_owned()),
            vtype: Set("int".to_owned()),
            text_val: Set(Some("oops".to_owned())),
            int_val: Set(None),
            real_val: Set(None),
        })
        .exec_without_returning(&db)
        .await;
        assert!(kv.is_err(), "vtype 与值列不符应被 CHECK 拒绝");
        let half = session_state::Entity::insert(session_state::ActiveModel {
            id: Set(0),
            cur_namespace: Set(Some("netease".to_owned())),
            cur_song_value: Set(None),
            position_ms: Set(0),
            play_mode: Set("sequential".to_owned()),
            volume: Set(1.0),
            updated_at: Set(0),
        })
        .exec_without_returning(&db)
        .await;
        assert!(half.is_err(), "当前曲半空对应被 CHECK 拒绝");
        let orphan = playlist_entries::Entity::insert(playlist_entries::ActiveModel {
            playlist_namespace: Set("netease".to_owned()),
            playlist_value: Set("nope".to_owned()),
            collection_index: Set(0),
            song_namespace: Set("netease".to_owned()),
            song_value: Set("s1".to_owned()),
        })
        .exec_without_returning(&db)
        .await;
        assert!(orphan.is_err(), "孤儿曲目行应被外键拒绝");
        Ok(())
    }

    /// server 的 down 删除全部表，随后 up 可重新建立可写入的数据库。
    #[tokio::test]
    async fn server_migration_up_down_up() -> color_eyre::Result<()> {
        let db = memory_database().await?;
        ServerMigrator::up(&db, /*steps*/ None).await?;
        song_favorites::Entity::insert(song_favorites::ActiveModel {
            namespace: Set("netease".to_owned()),
            song_value: Set("song".to_owned()),
            entered_at: Set(123456),
        })
        .exec_without_returning(&db)
        .await?;
        assert_eq!(song_favorites::Entity::find().count(&db).await?, 1);
        ServerMigrator::down(&db, /*steps*/ None).await?;
        assert!(song_favorites::Entity::find().count(&db).await.is_err());
        ServerMigrator::up(&db, /*steps*/ None).await?;
        assert_eq!(song_favorites::Entity::find().count(&db).await?, 0);
        Ok(())
    }

    /// client 的 up/down 可往返，回滚后偏好表被移除。
    #[tokio::test]
    async fn client_migration_up_down_up() -> color_eyre::Result<()> {
        let db = memory_database().await?;
        ClientMigrator::up(&db, /*steps*/ None).await?;
        assert_eq!(crate::entity::ui_prefs::Entity::find().count(&db).await?, 0);
        ClientMigrator::down(&db, /*steps*/ None).await?;
        assert!(
            crate::entity::ui_prefs::Entity::find()
                .count(&db)
                .await
                .is_err()
        );
        ClientMigrator::up(&db, /*steps*/ None).await?;
        assert_eq!(crate::entity::ui_prefs::Entity::find().count(&db).await?, 0);
        Ok(())
    }

    /// 无迁移记录的已有结构以迁移错误失败。
    #[tokio::test]
    async fn pre_migration_db_returns_migration_error() -> color_eyre::Result<()> {
        let db = memory_database().await?;
        db.execute(
            Table::create()
                .table(song_meta::Entity)
                .col(
                    ColumnDef::new(song_meta::Column::Namespace)
                        .text()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(song_meta::Column::SongValue)
                        .text()
                        .not_null(),
                )
                .col(ColumnDef::new(song_meta::Column::Name).text().not_null())
                .col(
                    ColumnDef::new(song_meta::Column::DurationMs)
                        .integer()
                        .not_null(),
                ),
        )
        .await?;
        let err = match ensure_schema(&db).await {
            Ok(()) => return Err(color_eyre::eyre::eyre!("老库应报错而非静默通过")),
            Err(error) => error,
        };
        assert!(matches!(
            err,
            crate::Error::Migration {
                database: "server",
                source: _
            }
        ));
        Ok(())
    }
}

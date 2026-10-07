//! 自建歌单及其有序歌曲身份关系。

use super::entity::{user_playlist_entries, user_playlists};
use mineral_log::info;
use sea_orm::sea_query::{Expr, ExprTrait, Table};
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

/// 自建歌单的初始表结构。
pub struct Migration;

// 迁移账本中的身份固定，不随实现文件的位置或名称改变。
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260930_user_playlists"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "starting database migration");
        let schema = Schema::new(manager.get_database_backend());
        let mut playlists = schema.create_table_from_entity(user_playlists::Entity);
        playlists.check(Expr::col(user_playlists::Column::TrackCount).gte(0));
        manager.create_table(playlists).await?;
        let mut entries = schema.create_table_from_entity(user_playlist_entries::Entity);
        entries.check(Expr::col(user_playlist_entries::Column::Position).gte(0));
        manager.create_table(entries).await?;
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "finished database migration statements");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "revert", "starting database migration");
        for table in [
            user_playlist_entries::Entity.table_ref(),
            user_playlists::Entity.table_ref(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        info!(target: "persist", database = "server", migration = self.name(), action = "revert", "finished database migration statements");
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

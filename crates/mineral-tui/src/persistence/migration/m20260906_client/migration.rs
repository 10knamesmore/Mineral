//! TUI 数据库的初始 Rust 结构版本。

use mineral_log::info;
use sea_orm::sea_query::Table;
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

use super::entity::{cover_cache, track_pos, ui_prefs};

/// 首次建库的固定结构。
pub struct Migration;

// 迁移账本中的身份固定，不随实现文件的位置或名称改变。
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260906_client"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "tui", migration = self.name(), action = "apply", "starting database migration");
        let schema = Schema::new(manager.get_database_backend());
        for table in [
            schema.create_table_from_entity(ui_prefs::Entity),
            schema.create_table_from_entity(track_pos::Entity),
            schema.create_table_from_entity(cover_cache::Entity),
        ] {
            manager.create_table(table).await?;
        }
        info!(target: "persist", database = "tui", migration = self.name(), action = "apply", "finished database migration statements");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "tui", migration = self.name(), action = "revert", "starting database migration");
        for table in [
            cover_cache::Entity.table_ref(),
            track_pos::Entity.table_ref(),
            ui_prefs::Entity.table_ref(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        info!(target: "persist", database = "tui", migration = self.name(), action = "revert", "finished database migration statements");
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

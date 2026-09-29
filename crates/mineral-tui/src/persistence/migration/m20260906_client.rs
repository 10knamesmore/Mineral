//! TUI 数据库的初始 Rust 结构版本。

use sea_orm::sea_query::Table;
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::prelude::DeriveMigrationName;
use sea_orm_migration::{MigrationTrait, SchemaManager};

use super::super::entity::{cover_cache, track_pos, ui_prefs};

/// 当前预发布结构的完整基线。
#[derive(DeriveMigrationName)]
pub(super) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        let schema = Schema::new(manager.get_database_backend());
        for table in [
            schema.create_table_from_entity(ui_prefs::Entity),
            schema.create_table_from_entity(track_pos::Entity),
            schema.create_table_from_entity(cover_cache::Entity),
        ] {
            manager.create_table(table).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        for table in [
            cover_cache::Entity.table_ref(),
            track_pos::Entity.table_ref(),
            ui_prefs::Entity.table_ref(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

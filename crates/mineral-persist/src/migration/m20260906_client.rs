//! client 数据库的初始 Rust 结构版本。

use sea_orm::DbErr;
use sea_orm::sea_query::Table;
use sea_orm_migration::prelude::DeriveMigrationName;
use sea_orm_migration::{MigrationTrait, SchemaManager};

use super::client_schema::{cover_cache, track_pos, ui_prefs};

/// 当前预发布结构的完整基线。
#[derive(DeriveMigrationName)]
pub(super) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        manager.create_table(ui_prefs::definition()).await?;
        for index in ui_prefs::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(track_pos::definition()).await?;
        for index in track_pos::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(cover_cache::definition()).await?;
        for index in cover_cache::indexes() {
            manager.create_index(index).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(cover_cache::CoverCache::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(track_pos::TrackPos::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ui_prefs::UiPrefs::Table).to_owned())
            .await?;
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

//! 自建歌单及其有序歌曲身份关系。

use super::super::entity::{user_playlist_entries, user_playlists};
use sea_orm::sea_query::{Expr, ExprTrait, ForeignKey, ForeignKeyAction, Table};
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::{MigrationTrait, SchemaManager, prelude::DeriveMigrationName};

/// 自建歌单的初始表结构。
#[derive(DeriveMigrationName)]
pub(super) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let schema = Schema::new(manager.get_database_backend());
        let mut playlists = schema.create_table_from_entity(user_playlists::Entity);
        playlists.check(Expr::col(user_playlists::Column::TrackCount).gte(0));
        manager.create_table(playlists).await?;
        let mut entries = schema.create_table_from_entity(user_playlist_entries::Entity);
        entries.check(Expr::col(user_playlist_entries::Column::Position).gte(0));
        // 只随歌单级联删除；来源元数据重建不得删除用户保存的歌曲身份。
        entries.foreign_key(
            ForeignKey::create()
                .from(
                    user_playlist_entries::Entity,
                    user_playlist_entries::Column::PlaylistId,
                )
                .to(user_playlists::Entity, user_playlists::Column::Id)
                .on_delete(ForeignKeyAction::Cascade),
        );
        manager.create_table(entries).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            user_playlist_entries::Entity.table_ref(),
            user_playlists::Entity.table_ref(),
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

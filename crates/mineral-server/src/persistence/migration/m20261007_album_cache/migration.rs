//! 新增完整专辑详情缓存

use mineral_log::info;
use sea_orm::sea_query::{Expr, ExprTrait, Table};
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

use super::entity::{albums, artists, track_artists, tracks};

/// 专辑详情缓存及其附属记录的初始结构。
pub struct Migration;

// 迁移账本中的身份固定，不随实现文件的位置或名称改变。
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261007_album_cache"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "starting database migration");
        let schema = Schema::new(manager.get_database_backend());
        let mut albums_table = schema.create_table_from_entity(albums::Entity);
        albums_table.check(Expr::col(albums::Column::TrackCount).gte(0));
        manager.create_table(albums_table).await?;

        let mut artists_table = schema.create_table_from_entity(artists::Entity);
        artists_table.check(Expr::col(artists::Column::Position).gte(0));
        manager.create_table(artists_table).await?;

        let mut tracks_table = schema.create_table_from_entity(tracks::Entity);
        tracks_table.check(Expr::col(tracks::Column::CollectionIndex).gte(0));
        tracks_table.check(Expr::col(tracks::Column::DurationMs).gte(0));
        tracks_table.check(
            Expr::col(tracks::Column::SongAlbumId)
                .is_null()
                .eq(Expr::col(tracks::Column::SongAlbumName).is_null()),
        );
        manager.create_table(tracks_table).await?;

        let mut track_artists_table = schema.create_table_from_entity(track_artists::Entity);
        track_artists_table.check(Expr::col(track_artists::Column::Position).gte(0));
        manager.create_table(track_artists_table).await?;
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "finished database migration statements");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "revert", "starting database migration");
        for table in [
            track_artists::Entity.table_ref(),
            tracks::Entity.table_ref(),
            artists::Entity.table_ref(),
            albums::Entity.table_ref(),
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

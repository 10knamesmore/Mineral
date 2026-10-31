use mineral_log::info;
use sea_orm::sea_query::{Expr, ExprTrait, Table};
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

use super::entity::{album_artists, albums, playlists, requested_albums, requested_playlists};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261030_library_list_cache"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "starting database migration");
        let schema = Schema::new(manager.get_database_backend());
        manager
            .create_table(schema.create_table_from_entity(requested_playlists::Entity))
            .await?;
        let mut playlists_table = schema.create_table_from_entity(playlists::Entity);
        playlists_table.check(Expr::col(playlists::Column::Position).gte(0));
        playlists_table.check(Expr::col(playlists::Column::TrackCount).gte(0));
        playlists_table.check(Expr::col(playlists::Column::PlayCount).gte(0));
        playlists_table.check(Expr::col(playlists::Column::SubscriberCount).gte(0));
        manager.create_table(playlists_table).await?;

        manager
            .create_table(schema.create_table_from_entity(requested_albums::Entity))
            .await?;
        let mut albums_table = schema.create_table_from_entity(albums::Entity);
        albums_table.check(Expr::col(albums::Column::Position).gte(0));
        albums_table.check(Expr::col(albums::Column::TrackCount).gte(0));
        manager.create_table(albums_table).await?;

        let mut artists_table = schema.create_table_from_entity(album_artists::Entity);
        artists_table.check(Expr::col(album_artists::Column::AlbumPosition).gte(0));
        artists_table.check(Expr::col(album_artists::Column::Position).gte(0));
        manager.create_table(artists_table).await?;
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "finished database migration statements");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "revert", "starting database migration");
        for table in [
            album_artists::Entity.table_ref(),
            albums::Entity.table_ref(),
            requested_albums::Entity.table_ref(),
            playlists::Entity.table_ref(),
            requested_playlists::Entity.table_ref(),
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

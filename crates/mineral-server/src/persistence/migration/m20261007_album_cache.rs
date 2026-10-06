//! 新增完整专辑详情缓存

use sea_orm::sea_query::{Expr, ExprTrait, ForeignKey, ForeignKeyAction, Table};
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::{MigrationTrait, SchemaManager, prelude::DeriveMigrationName};

use super::super::entity::album_cache::{albums, artists, track_artists, tracks};

#[derive(DeriveMigrationName)]
pub(super) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let schema = Schema::new(manager.get_database_backend());
        let mut albums_table = schema.create_table_from_entity(albums::Entity);
        albums_table.check(Expr::col(albums::Column::TrackCount).gte(0));
        manager.create_table(albums_table).await?;

        let mut artists_table = schema.create_table_from_entity(artists::Entity);
        artists_table.check(Expr::col(artists::Column::Position).gte(0));
        artists_table.foreign_key(
            ForeignKey::create()
                .from(
                    artists::Entity,
                    (artists::Column::Namespace, artists::Column::AlbumId),
                )
                .to(
                    albums::Entity,
                    (albums::Column::Namespace, albums::Column::AlbumId),
                )
                .on_delete(ForeignKeyAction::Cascade),
        );
        manager.create_table(artists_table).await?;

        let mut tracks_table = schema.create_table_from_entity(tracks::Entity);
        tracks_table.check(Expr::col(tracks::Column::CollectionIndex).gte(0));
        tracks_table.check(Expr::col(tracks::Column::DurationMs).gte(0));
        tracks_table.check(
            Expr::col(tracks::Column::SongAlbumId)
                .is_null()
                .eq(Expr::col(tracks::Column::SongAlbumName).is_null()),
        );
        tracks_table.foreign_key(
            ForeignKey::create()
                .from(
                    tracks::Entity,
                    (tracks::Column::Namespace, tracks::Column::AlbumId),
                )
                .to(
                    albums::Entity,
                    (albums::Column::Namespace, albums::Column::AlbumId),
                )
                .on_delete(ForeignKeyAction::Cascade),
        );
        manager.create_table(tracks_table).await?;

        let mut track_artists_table = schema.create_table_from_entity(track_artists::Entity);
        track_artists_table.check(Expr::col(track_artists::Column::Position).gte(0));
        track_artists_table.foreign_key(
            ForeignKey::create()
                .from(
                    track_artists::Entity,
                    (
                        track_artists::Column::Namespace,
                        track_artists::Column::AlbumId,
                        track_artists::Column::CollectionIndex,
                    ),
                )
                .to(
                    tracks::Entity,
                    (
                        tracks::Column::Namespace,
                        tracks::Column::AlbumId,
                        tracks::Column::CollectionIndex,
                    ),
                )
                .on_delete(ForeignKeyAction::Cascade),
        );
        manager.create_table(track_artists_table).await?;
        mineral_log::info!(target: "persist", "created album detail cache tables");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

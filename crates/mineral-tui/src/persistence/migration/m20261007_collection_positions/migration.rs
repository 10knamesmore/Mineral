//! 保留歌单位置，并为专辑提供独立的持久化身份。

use sea_orm::sea_query::{Expr, Query, Table};
use sea_orm::{ConnectionTrait, DbErr, ExprTrait, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

use super::entity::{collection_pos, playlist_pos};

/// 将旧歌单位置复制到带类型的新表。
pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261007_collection_positions"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        mineral_log::info!(target: "persist", database = "tui", "migrating collection cursor positions");
        let schema = Schema::new(manager.get_database_backend());
        let mut table = schema.create_table_from_entity(collection_pos::Entity);
        table.check(Expr::col(collection_pos::Column::CollectionKind).is_in(["playlist", "album"]));
        manager.create_table(table).await?;
        let copy = Query::insert()
            .into_table(collection_pos::Entity)
            .columns([
                collection_pos::Column::CollectionKind,
                collection_pos::Column::CollectionNamespace,
                collection_pos::Column::CollectionValue,
                collection_pos::Column::SongNamespace,
                collection_pos::Column::SongValue,
                collection_pos::Column::SelIndex,
                collection_pos::Column::ScreenRow,
            ])
            .select_from(
                Query::select()
                    .expr(Expr::val("playlist"))
                    .columns([
                        playlist_pos::Column::PlaylistNamespace,
                        playlist_pos::Column::PlaylistValue,
                        playlist_pos::Column::SongNamespace,
                        playlist_pos::Column::SongValue,
                        playlist_pos::Column::SelIndex,
                        playlist_pos::Column::ScreenRow,
                    ])
                    .from(playlist_pos::Entity)
                    .to_owned(),
            )
            .map_err(|error| DbErr::Migration(error.to_string()))?
            .to_owned();
        manager.execute(copy).await?;
        manager
            .drop_table(Table::drop().table(playlist_pos::Entity).to_owned())
            .await?;
        mineral_log::info!(target: "persist", database = "tui", "collection cursor positions migrated");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let query = Query::select()
            .expr(Expr::col(collection_pos::Column::CollectionKind).count())
            .from("collection_pos")
            .and_where(Expr::col(collection_pos::Column::CollectionKind).eq("album"))
            .to_owned();
        if let Some(row) = db.query_one(&query).await?
            && row.try_get_by_index::<i64>(0)? > 0
        {
            return Err(DbErr::Migration(
                "cannot remove collection type while album cursor positions exist".to_owned(),
            ));
        }
        let schema = Schema::new(manager.get_database_backend());
        manager
            .create_table(schema.create_table_from_entity(playlist_pos::Entity))
            .await?;
        let copy = Query::insert()
            .into_table(playlist_pos::Entity)
            .columns([
                playlist_pos::Column::PlaylistNamespace,
                playlist_pos::Column::PlaylistValue,
                playlist_pos::Column::SongNamespace,
                playlist_pos::Column::SongValue,
                playlist_pos::Column::SelIndex,
                playlist_pos::Column::ScreenRow,
            ])
            .select_from(
                Query::select()
                    .columns([
                        collection_pos::Column::CollectionNamespace,
                        collection_pos::Column::CollectionValue,
                        collection_pos::Column::SongNamespace,
                        collection_pos::Column::SongValue,
                        collection_pos::Column::SelIndex,
                        collection_pos::Column::ScreenRow,
                    ])
                    .from(collection_pos::Entity)
                    .to_owned(),
            )
            .map_err(|error| DbErr::Migration(error.to_string()))?
            .to_owned();
        manager.execute(copy).await?;
        manager
            .drop_table(Table::drop().table(collection_pos::Entity).to_owned())
            .await?;
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

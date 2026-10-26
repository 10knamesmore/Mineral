use mineral_log::info;
use sea_orm::sea_query::{Alias, Expr, ExprTrait, Query, Table, TableCreateStatement};
use sea_orm::{ConnectionTrait, DbErr, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

use super::entity::{albums, albums_with_required_release_date, artists, track_artists, tracks};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261026_album_release_date"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "starting database migration");
        rebuild_album_cache(manager, ReleaseDateRequirement::Optional).await?;
        // 旧模型用 0 表示缺失；升级后统一以 NULL 保存。
        manager
            .get_connection()
            .execute(
                Query::update()
                    .table(Alias::new("album_cache"))
                    .value(albums::Column::PublishTimeMs, None::<i64>)
                    .and_where(Expr::col(albums::Column::PublishTimeMs).eq(0)), // .to_owned(),
            )
            .await?;
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "finished database migration statements");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "revert", "starting database migration");
        let missing_release_date = Query::select()
            .column(albums::Column::AlbumId)
            .from(Alias::new("album_cache"))
            .and_where(Expr::col(albums::Column::PublishTimeMs).is_null())
            .limit(1)
            .to_owned();
        if manager
            .get_connection()
            .query_one(&missing_release_date)
            .await?
            .is_some()
        {
            return Err(DbErr::Migration(
                "Cannot revert m20261026_album_release_date: album_cache contains NULL publish_time_ms values"
                    .into(),
            ));
        }
        rebuild_album_cache(manager, ReleaseDateRequirement::Required).await?;
        info!(target: "persist", database = "server", migration = self.name(), action = "revert", "finished database migration statements");
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

#[derive(Clone, Copy)]
enum ReleaseDateRequirement {
    Optional,

    Required,
}

struct CacheTableBackup {
    table: &'static str,

    backup: &'static str,
}

// 父表在前：恢复按此顺序，删除按逆序，避免 DROP 父表触发级联删除。
const CACHE_TABLES: [CacheTableBackup; 4] = [
    CacheTableBackup {
        table: "album_cache",
        backup: "m20261026_album_release_date_albums",
    },
    CacheTableBackup {
        table: "album_cache_artists",
        backup: "m20261026_album_release_date_artists",
    },
    CacheTableBackup {
        table: "album_cache_tracks",
        backup: "m20261026_album_release_date_tracks",
    },
    CacheTableBackup {
        table: "album_cache_track_artists",
        backup: "m20261026_album_release_date_track_artists",
    },
];

/// 通过重建移除或恢复 NOT NULL，适用于项目锁定的 SQLite 3.46。
/// 四表在同一迁移事务内备份、重建和恢复，保留外键关系。
async fn rebuild_album_cache(
    manager: &SchemaManager<'_>,
    release_date: ReleaseDateRequirement,
) -> Result<(), DbErr> {
    let schema = Schema::new(manager.get_database_backend());
    let definitions = table_definitions(&schema, release_date);
    for (table, definition) in CACHE_TABLES.iter().zip(&definitions) {
        let mut backup = Table::create();
        backup.table(Alias::new(table.backup)).temporary();
        // 临时副本只复用快照字段，不带外键，避免删除原表时级联清掉备份。
        for column in definition.get_columns() {
            backup.col(column.clone());
        }
        manager.create_table(backup).await?;
        copy_rows(manager, table.table, table.backup, definition).await?;
    }
    for table in CACHE_TABLES.iter().rev() {
        manager
            .drop_table(Table::drop().table(Alias::new(table.table)).to_owned())
            .await?;
    }

    for definition in &definitions {
        manager.create_table(definition.clone()).await?;
    }
    for (table, definition) in CACHE_TABLES.iter().zip(&definitions) {
        copy_rows(manager, table.backup, table.table, definition).await?;
        manager
            .drop_table(Table::drop().table(Alias::new(table.backup)).to_owned())
            .await?;
    }
    Ok(())
}

fn table_definitions(
    schema: &Schema,
    release_date: ReleaseDateRequirement,
) -> [TableCreateStatement; 4] {
    let mut albums_table = match release_date {
        ReleaseDateRequirement::Optional => schema.create_table_from_entity(albums::Entity),
        ReleaseDateRequirement::Required => {
            schema.create_table_from_entity(albums_with_required_release_date::Entity)
        }
    };
    albums_table.check(Expr::col(albums::Column::TrackCount).gte(0));

    let mut artists_table = schema.create_table_from_entity(artists::Entity);
    artists_table.check(Expr::col(artists::Column::Position).gte(0));

    let mut tracks_table = schema.create_table_from_entity(tracks::Entity);
    tracks_table.check(Expr::col(tracks::Column::CollectionIndex).gte(0));
    tracks_table.check(Expr::col(tracks::Column::DurationMs).gte(0));
    tracks_table.check(
        Expr::col(tracks::Column::SongAlbumId)
            .is_null()
            .eq(Expr::col(tracks::Column::SongAlbumName).is_null()),
    );

    let mut track_artists_table = schema.create_table_from_entity(track_artists::Entity);
    track_artists_table.check(Expr::col(track_artists::Column::Position).gte(0));
    [
        albums_table,
        artists_table,
        tracks_table,
        track_artists_table,
    ]
}

async fn copy_rows(
    manager: &SchemaManager<'_>,
    source: &str,
    destination: &str,
    definition: &TableCreateStatement,
) -> Result<(), DbErr> {
    let columns: Vec<Alias> = definition
        .get_columns()
        .iter()
        .map(|column| Alias::new(column.get_column_name()))
        .collect();
    let select = Query::select()
        .columns(columns.clone())
        .from(Alias::new(source))
        .to_owned();
    let insert = Query::insert()
        .into_table(Alias::new(destination))
        .columns(columns)
        .select_from(select)
        .map_err(|error| DbErr::Migration(error.to_string()))?
        .to_owned();
    manager.get_connection().execute(&insert).await?;
    Ok(())
}

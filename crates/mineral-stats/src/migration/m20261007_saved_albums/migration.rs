//! 保留原有请求记录，扩展 fetches 的固定 CHECK 取值。

use sea_orm::sea_query::{Expr, Index, Query, Table};
use sea_orm::{DbErr, ExprTrait, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

use super::entity::fetches;

/// 为收藏专辑请求增加统计类型。
pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261007_saved_albums"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild(manager, true).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 旧约束无法表达新记录；INSERT 会失败并回滚，而不是删除这些历史。
        rebuild(manager, false).await
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

/// 事务内复制并替换表，所有字段及原索引名称保持不变。
async fn rebuild(manager: &SchemaManager<'_>, include_saved_albums: bool) -> Result<(), DbErr> {
    mineral_log::info!(target: "persist", database = "stats", include_saved_albums, "migrating saved album fetch statistics");
    let schema = Schema::new(manager.get_database_backend());
    let mut table = schema.create_table_from_entity(fetches::Entity);
    let mut kinds = vec![
        "my_playlists",
        "playlist_detail",
        "song_url",
        "lyrics",
        "remote_play_count",
        "search",
        "artist_detail",
        "artist_albums",
        "album_detail",
    ];
    if include_saved_albums {
        kinds.push("my_albums");
    }
    table.check(Expr::col(fetches::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(fetches::Column::FetchKind).is_in(kinds));
    table.check(Expr::col(fetches::Column::Trigger).is_in(["user", "system"]));
    table.check(Expr::col(fetches::Column::Outcome).is_in(["ok", "failed", "cancelled"]));
    manager.create_table(table).await?;
    let columns = [
        fetches::Column::Id,
        fetches::Column::Ts,
        fetches::Column::SessionId,
        fetches::Column::Actor,
        fetches::Column::FetchKind,
        fetches::Column::Source,
        fetches::Column::TargetRef,
        fetches::Column::Trigger,
        fetches::Column::Outcome,
        fetches::Column::LatencyMs,
    ];
    let copy = Query::insert()
        .into_table(fetches::Entity)
        .columns(columns)
        .select_from(Query::select().columns(columns).from("fetches").to_owned())
        .map_err(|error| DbErr::Migration(error.to_string()))?
        .to_owned();
    manager.execute(copy).await?;
    manager
        .drop_table(Table::drop().table("fetches").to_owned())
        .await?;
    manager
        .rename_table(Table::rename().table(fetches::Entity, "fetches").to_owned())
        .await?;
    manager
        .create_index(
            Index::create()
                .name("idx_fetches_ts")
                .table("fetches")
                .col("ts")
                .to_owned(),
        )
        .await?;
    mineral_log::info!(target: "persist", database = "stats", "saved album fetch statistics migrated");
    Ok(())
}

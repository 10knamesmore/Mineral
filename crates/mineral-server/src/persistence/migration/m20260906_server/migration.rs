//! 服务端数据库的初始结构。

use mineral_log::info;
use sea_orm::sea_query::{Expr, ExprTrait, Index, IndexOrder, Table, TableCreateStatement};
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

use super::entity::{
    audio_cache, playlist_cache, playlist_entries, session_queue, session_state, song_artists,
    song_envelope, song_favorites, song_kv, song_meta, song_stats,
};

/// 服务端首次建库的固定结构。
pub struct Migration;

// 迁移账本中的身份固定，不随实现文件的位置或名称改变。
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260906_server"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "starting database migration");
        let schema = Schema::new(manager.get_database_backend());
        for table in [
            schema.create_table_from_entity(song_meta::Entity),
            schema.create_table_from_entity(song_artists::Entity),
            schema.create_table_from_entity(playlist_cache::Entity),
            session_state_table(&schema),
            schema.create_table_from_entity(session_queue::Entity),
            song_kv_table(&schema),
            schema.create_table_from_entity(song_envelope::Entity),
            schema.create_table_from_entity(song_favorites::Entity),
            schema.create_table_from_entity(song_stats::Entity),
            playlist_entries_table(&schema),
            schema.create_table_from_entity(audio_cache::Entity),
        ] {
            manager.create_table(table).await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("idx_song_favorites_order")
                    .table(song_favorites::Entity)
                    .col((song_favorites::Column::EnteredAt, IndexOrder::Desc))
                    .col(song_favorites::Column::Namespace)
                    .col(song_favorites::Column::SongValue)
                    .to_owned(),
            )
            .await?;
        info!(target: "persist", database = "server", migration = self.name(), action = "apply", "finished database migration statements");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "server", migration = self.name(), action = "revert", "starting database migration");
        for table in [
            audio_cache::Entity.table_ref(),
            playlist_entries::Entity.table_ref(),
            song_stats::Entity.table_ref(),
            song_favorites::Entity.table_ref(),
            song_envelope::Entity.table_ref(),
            song_kv::Entity.table_ref(),
            session_queue::Entity.table_ref(),
            session_state::Entity.table_ref(),
            playlist_cache::Entity.table_ref(),
            song_artists::Entity.table_ref(),
            song_meta::Entity.table_ref(),
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

/// 限制会话为单例，并要求当前歌曲身份成对出现。
fn session_state_table(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(session_state::Entity);
    table.check(
        Expr::col(session_state::Column::CurNamespace)
            .is_null()
            .eq(Expr::col(session_state::Column::CurSongValue).is_null()),
    );
    table.check(Expr::col(session_state::Column::Id).eq(Expr::value(0i64)));
    table
}

/// 要求类型标签与唯一有值的载荷列一致。
fn song_kv_table(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(song_kv::Entity);
    table.check(
        Expr::col(song_kv::Column::Vtype)
            .is_in(["int", "bool"])
            .and(Expr::col(song_kv::Column::IntVal).is_null().not())
            .and(Expr::col(song_kv::Column::RealVal).is_null())
            .and(Expr::col(song_kv::Column::TextVal).is_null())
            .or(Expr::col(song_kv::Column::Vtype)
                .eq(Expr::value("real"))
                .and(Expr::col(song_kv::Column::RealVal).is_null().not())
                .and(Expr::col(song_kv::Column::IntVal).is_null())
                .and(Expr::col(song_kv::Column::TextVal).is_null()))
            .or(Expr::col(song_kv::Column::Vtype)
                .eq(Expr::value("text"))
                .and(Expr::col(song_kv::Column::TextVal).is_null().not())
                .and(Expr::col(song_kv::Column::IntVal).is_null())
                .and(Expr::col(song_kv::Column::RealVal).is_null())),
    );
    table
}

/// 限制条目位置非负，并随所属歌单删除条目。
fn playlist_entries_table(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(playlist_entries::Entity);
    table.check(Expr::col(playlist_entries::Column::CollectionIndex).gte(Expr::value(0i64)));
    table
}

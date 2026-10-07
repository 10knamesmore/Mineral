//! 统计数据库首次建库的固定结构。

use mineral_log::info;
use sea_orm::sea_query::Table;
use sea_orm::{DbErr, EntityName, Schema};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};

use super::entity::{
    action_invocations, app_lifecycle, bus_messages, cache_evictions, cache_harvests,
    client_connections, config_overrides, config_reloads, connection_rejects, copy_renders,
    downloads, fetches, fullscreen_changes, gapless_boundaries, hook_fires, love_changes,
    mode_changes, pauses, playlist_ops, plays, prefetches, queue_ops, script_lifecycle, searches,
    seeks, sessions, song_artists, songs, spawns, store_writes, stream_resolutions, volume_changes,
};
use super::{indexes, tables};

/// 统计数据库的初始迁移；结构定义由本模块独立保存。
pub struct Migration;

// 迁移账本中的身份固定，不随实现文件的位置或名称改变。
impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260906_stats"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "stats", migration = self.name(), action = "apply", "starting database migration");
        let schema = Schema::new(manager.get_database_backend());
        for table in tables::definitions(&schema) {
            manager.create_table(table).await?;
        }
        for index in indexes::definitions() {
            manager.create_index(index).await?;
        }
        info!(target: "persist", database = "stats", migration = self.name(), action = "apply", "finished database migration statements");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        info!(target: "persist", database = "stats", migration = self.name(), action = "revert", "starting database migration");
        for table in [
            prefetches::Entity.table_ref(),
            song_artists::Entity.table_ref(),
            queue_ops::Entity.table_ref(),
            client_connections::Entity.table_ref(),
            copy_renders::Entity.table_ref(),
            songs::Entity.table_ref(),
            config_reloads::Entity.table_ref(),
            script_lifecycle::Entity.table_ref(),
            cache_evictions::Entity.table_ref(),
            cache_harvests::Entity.table_ref(),
            gapless_boundaries::Entity.table_ref(),
            hook_fires::Entity.table_ref(),
            stream_resolutions::Entity.table_ref(),
            app_lifecycle::Entity.table_ref(),
            connection_rejects::Entity.table_ref(),
            fullscreen_changes::Entity.table_ref(),
            bus_messages::Entity.table_ref(),
            spawns::Entity.table_ref(),
            store_writes::Entity.table_ref(),
            config_overrides::Entity.table_ref(),
            action_invocations::Entity.table_ref(),
            downloads::Entity.table_ref(),
            fetches::Entity.table_ref(),
            playlist_ops::Entity.table_ref(),
            love_changes::Entity.table_ref(),
            mode_changes::Entity.table_ref(),
            volume_changes::Entity.table_ref(),
            pauses::Entity.table_ref(),
            seeks::Entity.table_ref(),
            searches::Entity.table_ref(),
            plays::Entity.table_ref(),
            sessions::Entity.table_ref(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        info!(target: "persist", database = "stats", migration = self.name(), action = "revert", "finished database migration statements");
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

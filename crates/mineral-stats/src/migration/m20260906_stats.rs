//! stats 数据库的初始 Rust 结构版本。

use sea_orm::DbErr;
use sea_orm::sea_query::Table;
use sea_orm_migration::prelude::DeriveMigrationName;
use sea_orm_migration::{MigrationTrait, SchemaManager};

use super::stats_schema::{
    action_invocations, app_lifecycle, bus_messages, cache_evictions, cache_harvests,
    client_connections, config_overrides, config_reloads, connection_rejects, copy_renders,
    downloads, fetches, fullscreen_changes, gapless_boundaries, hook_fires, love_changes,
    mode_changes, pauses, playlist_ops, plays, prefetches, queue_ops, script_lifecycle, searches,
    seeks, sessions, song_artists, songs, spawns, store_writes, stream_resolutions, volume_changes,
};

/// 当前预发布结构的完整基线。
#[derive(DeriveMigrationName)]
pub(super) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        manager.create_table(sessions::definition()).await?;
        for index in sessions::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(plays::definition()).await?;
        for index in plays::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(searches::definition()).await?;
        for index in searches::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(seeks::definition()).await?;
        for index in seeks::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(pauses::definition()).await?;
        for index in pauses::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(volume_changes::definition()).await?;
        for index in volume_changes::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(mode_changes::definition()).await?;
        for index in mode_changes::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(love_changes::definition()).await?;
        for index in love_changes::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(playlist_ops::definition()).await?;
        for index in playlist_ops::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(fetches::definition()).await?;
        for index in fetches::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(downloads::definition()).await?;
        for index in downloads::indexes() {
            manager.create_index(index).await?;
        }
        manager
            .create_table(action_invocations::definition())
            .await?;
        for index in action_invocations::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(config_overrides::definition()).await?;
        for index in config_overrides::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(store_writes::definition()).await?;
        for index in store_writes::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(spawns::definition()).await?;
        for index in spawns::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(bus_messages::definition()).await?;
        for index in bus_messages::indexes() {
            manager.create_index(index).await?;
        }
        manager
            .create_table(fullscreen_changes::definition())
            .await?;
        for index in fullscreen_changes::indexes() {
            manager.create_index(index).await?;
        }
        manager
            .create_table(connection_rejects::definition())
            .await?;
        for index in connection_rejects::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(app_lifecycle::definition()).await?;
        for index in app_lifecycle::indexes() {
            manager.create_index(index).await?;
        }
        manager
            .create_table(stream_resolutions::definition())
            .await?;
        for index in stream_resolutions::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(hook_fires::definition()).await?;
        for index in hook_fires::indexes() {
            manager.create_index(index).await?;
        }
        manager
            .create_table(gapless_boundaries::definition())
            .await?;
        for index in gapless_boundaries::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(cache_harvests::definition()).await?;
        for index in cache_harvests::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(cache_evictions::definition()).await?;
        for index in cache_evictions::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(script_lifecycle::definition()).await?;
        for index in script_lifecycle::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(config_reloads::definition()).await?;
        for index in config_reloads::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(songs::definition()).await?;
        for index in songs::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(copy_renders::definition()).await?;
        for index in copy_renders::indexes() {
            manager.create_index(index).await?;
        }
        manager
            .create_table(client_connections::definition())
            .await?;
        for index in client_connections::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(queue_ops::definition()).await?;
        for index in queue_ops::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(song_artists::definition()).await?;
        for index in song_artists::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(prefetches::definition()).await?;
        for index in prefetches::indexes() {
            manager.create_index(index).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(prefetches::Prefetches::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(song_artists::SongArtists::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(queue_ops::QueueOps::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(client_connections::ClientConnections::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(copy_renders::CopyRenders::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(songs::Songs::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(config_reloads::ConfigReloads::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(script_lifecycle::ScriptLifecycle::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(cache_evictions::CacheEvictions::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(cache_harvests::CacheHarvests::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(gapless_boundaries::GaplessBoundaries::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(hook_fires::HookFires::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(stream_resolutions::StreamResolutions::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(app_lifecycle::AppLifecycle::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(connection_rejects::ConnectionRejects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(fullscreen_changes::FullscreenChanges::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(bus_messages::BusMessages::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(spawns::Spawns::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(store_writes::StoreWrites::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(config_overrides::ConfigOverrides::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(action_invocations::ActionInvocations::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(downloads::Downloads::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(fetches::Fetches::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(playlist_ops::PlaylistOps::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(love_changes::LoveChanges::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(mode_changes::ModeChanges::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(volume_changes::VolumeChanges::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(pauses::Pauses::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(seeks::Seeks::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(searches::Searches::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(plays::Plays::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(sessions::Sessions::Table).to_owned())
            .await?;
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

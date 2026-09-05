//! server 数据库的初始 Rust 结构版本。

use sea_orm::DbErr;
use sea_orm::sea_query::Table;
use sea_orm_migration::prelude::DeriveMigrationName;
use sea_orm_migration::{MigrationTrait, SchemaManager};

use super::server_schema::{
    audio_cache, play_history, playlist_cache, playlist_entries, session_queue, session_state,
    song_artists, song_envelope, song_favorites, song_kv, song_meta, song_stats,
};

/// 当前预发布结构的完整基线。
#[derive(DeriveMigrationName)]
pub(super) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        manager.create_table(song_meta::definition()).await?;
        for index in song_meta::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(song_artists::definition()).await?;
        for index in song_artists::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(play_history::definition()).await?;
        for index in play_history::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(playlist_cache::definition()).await?;
        for index in playlist_cache::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(session_state::definition()).await?;
        for index in session_state::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(session_queue::definition()).await?;
        for index in session_queue::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(song_kv::definition()).await?;
        for index in song_kv::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(song_envelope::definition()).await?;
        for index in song_envelope::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(song_favorites::definition()).await?;
        for index in song_favorites::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(song_stats::definition()).await?;
        for index in song_stats::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(playlist_entries::definition()).await?;
        for index in playlist_entries::indexes() {
            manager.create_index(index).await?;
        }
        manager.create_table(audio_cache::definition()).await?;
        for index in audio_cache::indexes() {
            manager.create_index(index).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(audio_cache::AudioCache::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(playlist_entries::PlaylistEntries::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(song_stats::SongStats::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(song_favorites::SongFavorites::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(song_envelope::SongEnvelope::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(song_kv::SongKv::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(session_queue::SessionQueue::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(session_state::SessionState::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(playlist_cache::PlaylistCache::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(play_history::PlayHistory::Table)
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
            .drop_table(Table::drop().table(song_meta::SongMeta::Table).to_owned())
            .await?;
        Ok(())
    }

    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }
}

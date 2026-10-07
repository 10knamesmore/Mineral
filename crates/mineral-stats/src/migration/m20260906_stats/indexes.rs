//! 初始统计查询使用的命名索引。

use sea_orm::EntityName;
use sea_orm::sea_query::{Index, IndexCreateStatement};

use super::entity::{
    action_invocations, app_lifecycle, bus_messages, cache_evictions, cache_harvests,
    client_connections, config_overrides, config_reloads, connection_rejects, copy_renders,
    downloads, fetches, fullscreen_changes, gapless_boundaries, hook_fires, love_changes,
    mode_changes, pauses, playlist_ops, plays, prefetches, queue_ops, script_lifecycle, searches,
    seeks, song_artists, spawns, store_writes, stream_resolutions, volume_changes,
};

/// 保留事件时间索引、播放聚合索引和艺人查询索引。
pub(super) fn definitions() -> Vec<IndexCreateStatement> {
    let mut indexes = vec![
        Index::create()
            .name("idx_plays_context")
            .table(plays::Entity)
            .col(plays::Column::ContextKind)
            .col(plays::Column::ContextRef)
            .to_owned(),
        Index::create()
            .name("idx_plays_session")
            .table(plays::Entity)
            .col(plays::Column::SessionId)
            .to_owned(),
        Index::create()
            .name("idx_plays_song")
            .table(plays::Entity)
            .col(plays::Column::Ns)
            .col(plays::Column::SongValue)
            .col(plays::Column::StartedAt)
            .to_owned(),
        Index::create()
            .name("idx_plays_started")
            .table(plays::Entity)
            .col(plays::Column::StartedAt)
            .to_owned(),
        Index::create()
            .name("idx_song_artists_artist")
            .table(song_artists::Entity)
            .col(song_artists::Column::Ns)
            .col(song_artists::Column::ArtistValue)
            .to_owned(),
    ];
    for (name, table) in [
        ("idx_searches_ts", searches::Entity.table_ref()),
        ("idx_seeks_ts", seeks::Entity.table_ref()),
        ("idx_pauses_ts", pauses::Entity.table_ref()),
        ("idx_volume_changes_ts", volume_changes::Entity.table_ref()),
        ("idx_mode_changes_ts", mode_changes::Entity.table_ref()),
        ("idx_love_changes_ts", love_changes::Entity.table_ref()),
        ("idx_playlist_ops_ts", playlist_ops::Entity.table_ref()),
        ("idx_fetches_ts", fetches::Entity.table_ref()),
        ("idx_downloads_ts", downloads::Entity.table_ref()),
        (
            "idx_action_invocations_ts",
            action_invocations::Entity.table_ref(),
        ),
        (
            "idx_config_overrides_ts",
            config_overrides::Entity.table_ref(),
        ),
        ("idx_store_writes_ts", store_writes::Entity.table_ref()),
        ("idx_spawns_ts", spawns::Entity.table_ref()),
        ("idx_bus_messages_ts", bus_messages::Entity.table_ref()),
        (
            "idx_fullscreen_changes_ts",
            fullscreen_changes::Entity.table_ref(),
        ),
        (
            "idx_connection_rejects_ts",
            connection_rejects::Entity.table_ref(),
        ),
        ("idx_app_lifecycle_ts", app_lifecycle::Entity.table_ref()),
        (
            "idx_stream_resolutions_ts",
            stream_resolutions::Entity.table_ref(),
        ),
        ("idx_hook_fires_ts", hook_fires::Entity.table_ref()),
        (
            "idx_gapless_boundaries_ts",
            gapless_boundaries::Entity.table_ref(),
        ),
        ("idx_cache_harvests_ts", cache_harvests::Entity.table_ref()),
        (
            "idx_cache_evictions_ts",
            cache_evictions::Entity.table_ref(),
        ),
        (
            "idx_script_lifecycle_ts",
            script_lifecycle::Entity.table_ref(),
        ),
        ("idx_config_reloads_ts", config_reloads::Entity.table_ref()),
        ("idx_copy_renders_ts", copy_renders::Entity.table_ref()),
        (
            "idx_client_connections_ts",
            client_connections::Entity.table_ref(),
        ),
        ("idx_queue_ops_ts", queue_ops::Entity.table_ref()),
        ("idx_prefetches_ts", prefetches::Entity.table_ref()),
    ] {
        indexes.push(Index::create().name(name).table(table).col("ts").to_owned());
    }
    indexes
}

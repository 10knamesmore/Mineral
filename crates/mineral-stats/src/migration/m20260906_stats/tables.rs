//! 初始统计表及其固定的取值约束。

use sea_orm::Schema;
use sea_orm::sea_query::{Expr, ExprTrait, TableCreateStatement};

use super::entity::{
    action_invocations, app_lifecycle, bus_messages, cache_evictions, cache_harvests,
    client_connections, config_overrides, config_reloads, connection_rejects, copy_renders,
    downloads, fetches, fullscreen_changes, gapless_boundaries, hook_fires, love_changes,
    mode_changes, pauses, playlist_ops, plays, prefetches, queue_ops, script_lifecycle, searches,
    seeks, sessions, song_artists, songs, spawns, store_writes, stream_resolutions, volume_changes,
};

/// 按外键依赖顺序生成初始表结构。
pub(super) fn definitions(schema: &Schema) -> Vec<TableCreateStatement> {
    vec![
        schema.create_table_from_entity(sessions::Entity),
        plays(schema),
        searches(schema),
        seeks(schema),
        pauses(schema),
        volume_changes(schema),
        mode_changes(schema),
        love_changes(schema),
        playlist_ops(schema),
        fetches(schema),
        downloads(schema),
        action_invocations(schema),
        config_overrides(schema),
        store_writes(schema),
        spawns(schema),
        bus_messages(schema),
        fullscreen_changes(schema),
        connection_rejects(schema),
        app_lifecycle(schema),
        stream_resolutions(schema),
        hook_fires(schema),
        gapless_boundaries(schema),
        cache_harvests(schema),
        schema.create_table_from_entity(cache_evictions::Entity),
        script_lifecycle(schema),
        schema.create_table_from_entity(config_reloads::Entity),
        schema.create_table_from_entity(songs::Entity),
        copy_renders(schema),
        client_connections(schema),
        queue_ops(schema),
        schema.create_table_from_entity(song_artists::Entity),
        prefetches(schema),
    ]
}

/// 固定 plays 的初始取值范围。
fn plays(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(plays::Entity);
    table.check(Expr::col(plays::Column::FinishReason).is_in(["eof", "skip", "stop", "error"]));
    table.check(Expr::col(plays::Column::PlayMode).is_in([
        "sequential",
        "shuffle",
        "repeat_all",
        "repeat_one",
    ]));
    table.check(Expr::col(plays::Column::OriginKind).is_in([
        "explicit",
        "auto_advance",
        "resume",
        "script",
        "unknown",
    ]));
    table.check(Expr::col(plays::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(
        Expr::col(plays::Column::ContextKind)
            .is_in(["search", "playlist", "album", "artist", "manual", "unknown"]),
    );
    table.check(
        Expr::col(plays::Column::Quality)
            .is_in(["standard", "higher", "exhigh", "lossless", "hires"]),
    );
    table.check(Expr::col(plays::Column::PlaybackOrigin).is_in(["download", "cache", "remote"]));
    table
}

/// 固定 searches 的初始取值范围。
fn searches(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(searches::Entity);
    table.check(Expr::col(searches::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(searches::Column::Kind).is_in(["song", "album", "artist", "playlist"]));
    table.check(Expr::col(searches::Column::Outcome).is_in(["ok", "failed", "cancelled"]));
    table
}

/// 固定 seeks 的初始取值范围。
fn seeks(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(seeks::Entity);
    table.check(Expr::col(seeks::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table
}

/// 固定 pauses 的初始取值范围。
fn pauses(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(pauses::Entity);
    table.check(Expr::col(pauses::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(pauses::Column::Action).is_in(["pause", "resume"]));
    table
}

/// 固定 volume_changes 的初始取值范围。
fn volume_changes(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(volume_changes::Entity);
    table
        .check(Expr::col(volume_changes::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table
}

/// 固定 mode_changes 的初始取值范围。
fn mode_changes(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(mode_changes::Entity);
    table.check(Expr::col(mode_changes::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(mode_changes::Column::FromMode).is_in([
        "sequential",
        "shuffle",
        "repeat_all",
        "repeat_one",
    ]));
    table.check(Expr::col(mode_changes::Column::ToMode).is_in([
        "sequential",
        "shuffle",
        "repeat_all",
        "repeat_one",
    ]));
    table
}

/// 固定 love_changes 的初始取值范围。
fn love_changes(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(love_changes::Entity);
    table.check(Expr::col(love_changes::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(love_changes::Column::Origin).is_in(["user", "import"]));
    table.check(Expr::col(love_changes::Column::RemoteMirror).is_in([
        "ok",
        "not_supported",
        "failed",
    ]));
    table
}

/// 固定 playlist_ops 的初始取值范围。
fn playlist_ops(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(playlist_ops::Entity);
    table.check(Expr::col(playlist_ops::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(playlist_ops::Column::Op).is_in([
        "create",
        "delete",
        "add",
        "remove",
        "rename",
        "set_description",
    ]));
    table.check(Expr::col(playlist_ops::Column::Outcome).is_in(["ok", "failed"]));
    table.check(Expr::col(playlist_ops::Column::ErrorKind).is_in([
        "auth_required",
        "rate_limited",
        "not_supported",
        "api",
        "other",
    ]));
    table
}

/// 固定 fetches 的初始取值范围。
fn fetches(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(fetches::Entity);
    table.check(Expr::col(fetches::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(fetches::Column::FetchKind).is_in([
        "my_playlists",
        "playlist_detail",
        "song_url",
        "lyrics",
        "remote_play_count",
        "search",
        "artist_detail",
        "artist_albums",
        "album_detail",
    ]));
    table.check(Expr::col(fetches::Column::Trigger).is_in(["user", "system"]));
    table.check(Expr::col(fetches::Column::Outcome).is_in(["ok", "failed", "cancelled"]));
    table
}

/// 固定 downloads 的初始取值范围。
fn downloads(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(downloads::Entity);
    table.check(Expr::col(downloads::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(downloads::Column::Outcome).is_in(["downloaded", "skipped", "failed"]));
    table.check(Expr::col(downloads::Column::Hooked).is_in(["none", "rewrite", "skip"]));
    table
}

/// 固定 action_invocations 的初始取值范围。
fn action_invocations(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(action_invocations::Entity);
    table.check(
        Expr::col(action_invocations::Column::Actor).is_in(["user", "script", "system", "cli"]),
    );
    table.check(Expr::col(action_invocations::Column::Trigger).is_in(["tui", "cli"]));
    table.check(Expr::col(action_invocations::Column::Outcome).is_in(["ok", "failed"]));
    table
}

/// 固定 config_overrides 的初始取值范围。
fn config_overrides(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(config_overrides::Entity);
    table.check(
        Expr::col(config_overrides::Column::Actor).is_in(["user", "script", "system", "cli"]),
    );
    table
}

/// 固定 store_writes 的初始取值范围。
fn store_writes(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(store_writes::Entity);
    table.check(Expr::col(store_writes::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(store_writes::Column::Op).is_in(["set", "inc"]));
    table
}

/// 固定 spawns 的初始取值范围。
fn spawns(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(spawns::Entity);
    table.check(Expr::col(spawns::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(spawns::Column::Outcome).is_in(["exited", "killed", "spawn_failed"]));
    table
}

/// 固定 bus_messages 的初始取值范围。
fn bus_messages(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(bus_messages::Entity);
    table.check(Expr::col(bus_messages::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table
}

/// 固定 fullscreen_changes 的初始取值范围。
fn fullscreen_changes(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(fullscreen_changes::Entity);
    table.check(
        Expr::col(fullscreen_changes::Column::Actor).is_in(["user", "script", "system", "cli"]),
    );
    table
}

/// 固定 connection_rejects 的初始取值范围。
fn connection_rejects(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(connection_rejects::Entity);
    table.check(
        Expr::col(connection_rejects::Column::Actor).is_in(["user", "script", "system", "cli"]),
    );
    table.check(Expr::col(connection_rejects::Column::Reason).is_in(["busy", "version_mismatch"]));
    table
}

/// 固定 app_lifecycle 的初始取值范围。
fn app_lifecycle(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(app_lifecycle::Entity);
    table.check(Expr::col(app_lifecycle::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(app_lifecycle::Column::Who).is_in(["daemon", "client"]));
    table.check(Expr::col(app_lifecycle::Column::Phase).is_in(["start", "stop"]));
    table.check(Expr::col(app_lifecycle::Column::AudioBackend).is_in(["device", "null"]));
    table
}

/// 固定 stream_resolutions 的初始取值范围。
fn stream_resolutions(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(stream_resolutions::Entity);
    table.check(Expr::col(stream_resolutions::Column::Outcome).is_in(["ok", "empty", "error"]));
    table
}

/// 固定 hook_fires 的初始取值范围。
fn hook_fires(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(hook_fires::Entity);
    table.check(Expr::col(hook_fires::Column::Hook).is_in(["before_stream", "before_download"]));
    table.check(Expr::col(hook_fires::Column::Stage).is_in(["immediate", "prefetch"]));
    table.check(Expr::col(hook_fires::Column::Decision).is_in(["continue", "rewrite", "skip"]));
    table.check(Expr::col(hook_fires::Column::FailOpen).is_in(["timeout", "thread_dead", "error"]));
    table
}

/// 固定 gapless_boundaries 的初始取值范围。
fn gapless_boundaries(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(gapless_boundaries::Entity);
    table.check(Expr::col(gapless_boundaries::Column::Result).is_in(["adopt", "fallback"]));
    table
}

/// 固定 cache_harvests 的初始取值范围。
fn cache_harvests(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(cache_harvests::Entity);
    table.check(Expr::col(cache_harvests::Column::Outcome).is_in(["cached", "discarded"]));
    table
}

/// 固定 script_lifecycle 的初始取值范围。
fn script_lifecycle(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(script_lifecycle::Entity);
    table.check(Expr::col(script_lifecycle::Column::Event).is_in([
        "load",
        "reload_ok",
        "reload_fail",
        "callback_error",
        "watchdog_abort",
        "config_warning",
    ]));
    table
}

/// 固定 copy_renders 的初始取值范围。
fn copy_renders(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(copy_renders::Entity);
    table.check(Expr::col(copy_renders::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(
        Expr::col(copy_renders::Column::CtxKind).is_in(["song", "playlist", "album", "artist"]),
    );
    table.check(Expr::col(copy_renders::Column::Outcome).is_in(["ok", "failed"]));
    table
}

/// 固定 client_connections 的初始取值范围。
fn client_connections(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(client_connections::Entity);
    table.check(
        Expr::col(client_connections::Column::Actor).is_in(["user", "script", "system", "cli"]),
    );
    table
}

/// 固定 queue_ops 的初始取值范围。
fn queue_ops(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(queue_ops::Entity);
    table.check(Expr::col(queue_ops::Column::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(Expr::col(queue_ops::Column::Op).is_in([
        "set",
        "insert_next",
        "append",
        "clear",
        "remove",
        "move",
        "clear_above",
        "clear_below",
        "transform",
        "undo",
    ]));
    table
}

/// 固定 prefetches 的初始取值范围。
fn prefetches(schema: &Schema) -> TableCreateStatement {
    let mut table = schema.create_table_from_entity(prefetches::Entity);
    table.check(Expr::col(prefetches::Column::Source).is_in(["local", "remote"]));
    table.check(Expr::col(prefetches::Column::Resolution).is_in([
        "armed",
        "vetoed",
        "rewritten",
        "failed",
    ]));
    table
}

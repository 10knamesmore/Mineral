//! Domain model 到 Lua table 的边缘 projection。

use mineral_model::Song;
use mlua::Lua;

/// `Song` 在 Lua 侧的 projection。
///
/// id 用 `qualified()`；`duration_ms` 时长未知时为 nil。该裸 Song table 不携带
/// collection-only index，供 queue/search 等 non-collection API 共用。
pub(crate) fn song_table(lua: &Lua, song: &Song) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.set("id", song.id.qualified())?;
    table.set("title", song.name.clone())?;
    table.set("duration_ms", song.duration_ms)?;
    table.set(
        "artists",
        lua.create_sequence_from(song.artists.iter().map(|artist| artist.name.clone()))?,
    )?;
    table.set("album", song.album.as_ref().map(|album| album.name.clone()))?;
    table.set(
        "cover_url",
        song.cover_url.as_ref().map(ToString::to_string),
    )?;
    table.set(
        "source_url",
        song.source_url.as_ref().map(ToString::to_string),
    )?;
    table.set("source", song.source().name())?;
    table.set(
        "url",
        web_url(
            lua,
            song.source().name(),
            /*kind*/ "song",
            song.id.value(),
        ),
    )?;
    Ok(table)
}

/// `PlaylistEntry` 在 Lua collection consumer 侧的 projection。
pub(super) fn playlist_entry_table(
    lua: &Lua,
    entry: &mineral_model::PlaylistEntry,
) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.set("index", entry.index.get())?;
    table.set("song", song_table(lua, &entry.song)?)?;
    Ok(table)
}

/// `Playlist` 在 Lua 复制模板侧的 projection。
pub(super) fn playlist_table(
    lua: &Lua,
    playlist: &mineral_model::Playlist,
) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.set("id", playlist.id.qualified())?;
    table.set("name", playlist.name.clone())?;
    table.set("description", playlist.description.clone())?;
    table.set("track_count", playlist.track_count)?;
    table.set(
        "cover_url",
        playlist.cover_url.as_ref().map(ToString::to_string),
    )?;
    table.set("source", playlist.source().name())?;
    table.set(
        "url",
        web_url(
            lua,
            playlist.source().name(),
            /*kind*/ "playlist",
            playlist.id.value(),
        ),
    )?;
    let songs = lua.create_table()?;
    for (i, entry) in playlist.entries.iter().enumerate() {
        songs.raw_set(i + 1, song_table(lua, &entry.song)?)?;
    }
    table.set("songs", songs)?;
    Ok(table)
}

/// `Album` 在 Lua 复制模板侧的 projection。
pub(super) fn album_table(lua: &Lua, album: &mineral_model::Album) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.set("id", album.id.qualified())?;
    table.set("name", album.name.clone())?;
    table.set(
        "artists",
        lua.create_sequence_from(album.artists.iter().map(|artist| artist.name.clone()))?,
    )?;
    table.set("description", album.description.clone())?;
    table.set("track_count", album.track_count)?;
    table.set(
        "cover_url",
        album.cover_url.as_ref().map(ToString::to_string),
    )?;
    table.set("source", album.source().name())?;
    table.set(
        "url",
        web_url(
            lua,
            album.source().name(),
            /*kind*/ "album",
            album.id.value(),
        ),
    )?;
    let songs = lua.create_table()?;
    for (i, track) in album.tracks.iter().enumerate() {
        songs.raw_set(i + 1, song_table(lua, &track.song)?)?;
    }
    table.set("songs", songs)?;
    Ok(table)
}

/// `Artist` 在 Lua 复制模板侧的 projection。
pub(super) fn artist_table(lua: &Lua, artist: &mineral_model::Artist) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.set("id", artist.id.qualified())?;
    table.set("name", artist.name.clone())?;
    table.set("description", artist.description.clone())?;
    table.set("follower_count", artist.follower_count)?;
    table.set("album_count", artist.album_count)?;
    table.set("song_count", artist.song_count)?;
    table.set(
        "avatar_url",
        artist.avatar_url.as_ref().map(ToString::to_string),
    )?;
    table.set("source", artist.source().name())?;
    table.set(
        "url",
        web_url(
            lua,
            artist.source().name(),
            /*kind*/ "artist",
            artist.id.value(),
        ),
    )?;
    let songs = lua.create_table()?;
    for (i, song) in artist.songs.iter().enumerate() {
        songs.raw_set(i + 1, song_table(lua, song)?)?;
    }
    table.set("songs", songs)?;
    Ok(table)
}

/// 按 source seed 拼网页分享链接；未配置时返回 `None`。
fn web_url(lua: &Lua, source: &str, kind: &str, raw_id: &str) -> Option<String> {
    let table: mlua::Table = lua
        .named_registry_value(crate::host::WEB_URL_TEMPLATES)
        .ok()?;
    let entry: mlua::Table = table.get(source).ok()?;
    let template: String = entry.get(kind).ok()?;
    Some(mineral_channel_core::render_web_url(&template, raw_id))
}

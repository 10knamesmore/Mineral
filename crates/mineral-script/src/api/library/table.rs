//! 组装曲库查询与喜欢状态的 Lua API 表。

use mlua::{Lua, Table};

use super::{love, playlists, search, song_url, tracks};
use crate::host::ScriptHost;

/// 组装 `library` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let library = lua.create_table()?;
    playlists::install(lua, &library, host)?;
    tracks::install(lua, &library, host)?;
    search::install(lua, &library, host)?;
    song_url::install(lua, &library, host)?;
    love::install(lua, &library, host)?;
    mineral.set("library", library)
}

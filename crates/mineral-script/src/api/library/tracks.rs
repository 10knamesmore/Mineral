//! `mineral.library.tracks(playlist_id, fn)`:读指定歌单的曲目(回调风格)。

use mlua::{Lua, Table};

use crate::api::value::parse_playlist_id;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `tracks` 挂到 `library` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `library`: `mineral.library` 子表
///   - `host`: 宿主句柄(命令出口 + 在途查询表)
pub(crate) fn install(lua: &Lua, library: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    library.set(
        "tracks",
        lua.create_function(
            move |lua, (playlist_id, callback): (String, mlua::Function)| {
                let playlist = parse_playlist_id(&playlist_id)?;
                let query = h.register_query(lua, callback)?;
                let _ = h
                    .commands
                    .send(ScriptCmd::LibraryTracks { playlist, query });
                Ok(())
            },
        )?,
    )
}

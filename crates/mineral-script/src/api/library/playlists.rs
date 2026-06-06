//! `mineral.library.playlists(fn)`:读用户歌单列表(回调风格,跨源聚合)。

use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `playlists` 挂到 `library` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `library`: `mineral.library` 子表
///   - `host`: 宿主句柄(命令出口 + 在途查询表)
pub(crate) fn install(lua: &Lua, library: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    library.set(
        "playlists",
        lua.create_function(move |lua, callback: mlua::Function| {
            let query = h.register_query(lua, callback)?;
            let _ = h.commands.send(ScriptCmd::LibraryPlaylists { query });
            Ok(())
        })?,
    )
}

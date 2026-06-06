//! `mineral.library.love(song_id, loved)`:设/取消一首歌的 ♥
//! (fire-and-forget;本地 persist + 远端 channel)。

use mlua::{Lua, Table};

use crate::api::value::parse_song_id;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `love` 挂到 `library` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `library`: `mineral.library` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, library: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    library.set(
        "love",
        lua.create_function(move |_lua, (song_id, loved): (String, bool)| {
            let song = parse_song_id(&song_id)?;
            let _ = h.commands.send(ScriptCmd::SetLoved { song, loved });
            Ok(())
        })?,
    )
}

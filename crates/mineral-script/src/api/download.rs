//! `mineral.download(song_id)`:下载指定歌曲(fire-and-forget)。

use mlua::{Lua, Table};

use crate::api::value::parse_song_id;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `download` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    mineral.set(
        "download",
        lua.create_function(move |_lua, song_id: String| {
            let _ = commands.send(ScriptCmd::Download(parse_song_id(&song_id)?));
            Ok(())
        })?,
    )
}

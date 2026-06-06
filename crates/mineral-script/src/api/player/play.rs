//! `mineral.player.play(song_id)`:播放指定歌曲(当前限队列内跳播)。

use mlua::{Lua, Table};

use crate::api::value::parse_song_id;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `play` 挂到 `player` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `player`: `mineral.player` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, player: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    player.set(
        "play",
        lua.create_function(move |_lua, song_id: String| {
            let _ = commands.send(ScriptCmd::Play(parse_song_id(&song_id)?));
            Ok(())
        })?,
    )
}

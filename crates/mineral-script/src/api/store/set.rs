//! `mineral.store.set(song_id, key, value)`:写 per-song 持久值
//! (fire-and-forget;`nil` 删除该 key;保留键由 persist 层拒写)。

use mlua::{Lua, Table};

use crate::api::value::{lua_to_store, parse_song_id, warn_unprefixed_key};
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `set` 挂到 `store` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `store`: `mineral.store` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, store: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    store.set(
        "set",
        lua.create_function(
            move |_lua, (song_id, key, value): (String, String, mlua::Value)| {
                let song = parse_song_id(&song_id)?;
                warn_unprefixed_key(&key);
                let value = lua_to_store(&value)?;
                let _ = h.commands.send(ScriptCmd::StoreSet { song, key, value });
                Ok(())
            },
        )?,
    )
}

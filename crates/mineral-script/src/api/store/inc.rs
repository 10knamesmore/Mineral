//! `mineral.store.inc(song_id, key, delta, fn?)`:per-song 数值自增。
//!
//! key 不存在以 delta 起步;现有值非整数报错。带回调时经查询回投
//! 拿到自增后的值,不带则 fire-and-forget(失败只记日志)。

use mlua::{Lua, Table};

use crate::api::value::{parse_song_id, warn_unprefixed_key};
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `inc` 挂到 `store` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `store`: `mineral.store` 子表
///   - `host`: 宿主句柄(命令出口 + 在途查询表)
pub(crate) fn install(lua: &Lua, store: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    store.set(
        "inc",
        lua.create_function(
            move |lua,
                  (song_id, key, delta, callback): (
                String,
                String,
                i64,
                Option<mlua::Function>,
            )| {
                let song = parse_song_id(&song_id)?;
                warn_unprefixed_key(&key);
                let query = callback.map(|cb| h.register_query(lua, cb)).transpose()?;
                let _ = h.commands.send(ScriptCmd::StoreInc {
                    song,
                    key,
                    delta,
                    query,
                });
                Ok(())
            },
        )?,
    )
}

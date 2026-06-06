//! `mineral.store.get(song_id, key, fn)`:读 per-song 持久值(回调风格)。
//!
//! 脚本线程不阻塞等 sqlite:回调挂 pending 表、命令带 [`crate::QueryId`]
//! 出去,daemon 泵查完回投,结果作为脚本循环里的一条消息再调回调
//! (看门狗对每段回调独立计时)。

use mlua::{Lua, Table};

use crate::api::value::parse_song_id;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `get` 挂到 `store` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `store`: `mineral.store` 子表
///   - `host`: 宿主句柄(命令出口 + 在途查询表)
pub(crate) fn install(lua: &Lua, store: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    store.set(
        "get",
        lua.create_function(
            move |lua, (song_id, key, callback): (String, String, mlua::Function)| {
                let song = parse_song_id(&song_id)?;
                let query = h.register_query(lua, callback)?;
                let _ = h.commands.send(ScriptCmd::StoreGet { song, key, query });
                Ok(())
            },
        )?,
    )
}

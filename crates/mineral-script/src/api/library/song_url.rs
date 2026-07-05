//! `mineral.library.song_url(id, fn)`:解析一首歌的可播 URL(回调风格)。
//!
//! 按 id 的 namespace 找对应 channel 取流;回调收的投影字段与 hook 改写返回值
//! 对齐(`url` / `quality` / `headers` / `layout`),可原样喂给 `ctx.resolve(...)`。

use mlua::{Lua, Table};

use crate::api::value::parse_song_id;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `song_url` 挂到 `library` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `library`: `mineral.library` 子表
///   - `host`: 宿主句柄(命令出口 + 在途查询表)
pub(crate) fn install(lua: &Lua, library: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    library.set(
        "song_url",
        lua.create_function(move |lua, (raw, callback): (String, mlua::Function)| {
            let song = parse_song_id(&raw)?;
            let query = h.register_query(lua, callback)?;
            let _ = h.commands.send(ScriptCmd::LibrarySongUrl { song, query });
            Ok(())
        })?,
    )
}

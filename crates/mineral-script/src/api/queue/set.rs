//! `mineral.queue.set(songs)`:整表重排队列。
//!
//! 收一个歌表数组(通常是 `mineral.queue.list()` 的返回值经脚本变换后的结果),只读每项的
//! `id`。每个 id 必须在当前队列里出现过,次数不限——复制已在队列的歌零成本;混入外来 id
//! 则整次重排被 daemon 拒绝、队列不动。

use mlua::{Lua, Table};

use crate::api::value::parse_song_id;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `set` 挂到 `queue` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `queue`: `mineral.queue` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, queue: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    queue.set(
        "set",
        lua.create_function(move |_lua, songs: Table| {
            let mut ids = Vec::with_capacity(songs.raw_len());
            for at in 1..=songs.raw_len() {
                let entry: Table = songs.get(at).map_err(|_not_a_table| {
                    mlua::Error::RuntimeError(format!("queue.set: 第 {at} 项不是歌表"))
                })?;
                let qualified: String = entry.get("id").map_err(|_missing| {
                    mlua::Error::RuntimeError(format!("queue.set: 第 {at} 项缺 id 字段"))
                })?;
                ids.push(parse_song_id(&qualified)?);
            }
            let _ = commands.send(ScriptCmd::QueueSet { ids });
            Ok(())
        })?,
    )
}

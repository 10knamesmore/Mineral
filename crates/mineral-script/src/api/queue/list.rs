//! `mineral.queue.list(fn)`:读当前播放队列(回调风格,数组顺序即队列顺序)。
//!
//! 队列**编辑**(add / remove / 稳定句柄寻址)是规划中的 native 能力,
//! native 落地后再开脚本面 —— 出参 table 加字段(如 entry 句柄)零破坏。
//! 跳播用现有 `mineral.player.play(song_id)`(队列内查找)。

use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `list` 挂到 `queue` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `queue`: `mineral.queue` 子表
///   - `host`: 宿主句柄(命令出口 + 在途查询表)
pub(crate) fn install(lua: &Lua, queue: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    queue.set(
        "list",
        lua.create_function(move |lua, callback: mlua::Function| {
            let query = h.register_query(lua, callback)?;
            let _ = h.commands.send(ScriptCmd::QueueList { query });
            Ok(())
        })?,
    )
}

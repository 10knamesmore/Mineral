//! `mineral.timer.*`:脚本线程内定时器。
//! 一个文件对应一个 Lua 函数,与脚本侧 API 树一一对应;承载结构在 `table`。
//!
//! 不另起线程 / tokio:挂在脚本线程主循环的 `recv_timeout` 心跳上
//! (无定时器时长等消息,零空转)。回调与事件回调同走看门狗熔断。
//! (端到端行为测试在 `runtime.rs`:真脚本线程 + 真实时间。)

pub(crate) mod after;
pub(crate) mod every;
pub(crate) mod table;

use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 组装 `timer` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(共享定时器表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let timer = lua.create_table()?;
    after::install(lua, &timer, host)?;
    every::install(lua, &timer, host)?;
    mineral.set("timer", timer)
}

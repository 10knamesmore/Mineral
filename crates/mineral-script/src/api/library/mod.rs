//! `mineral.library.*`:用户曲库的脚本出口(映射 channel 能力)。
//! 一个文件对应一个 Lua 函数,与脚本侧 API 树一一对应。
//! (端到端行为测试在 `runtime.rs`:真脚本线程 + 模拟 daemon 泵回投。)

pub(crate) mod love;
pub(crate) mod playlists;
pub(crate) mod search;
pub(crate) mod song_url;
pub(crate) mod tracks;

use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 组装 `library` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let library = lua.create_table()?;
    playlists::install(lua, &library, host)?;
    tracks::install(lua, &library, host)?;
    search::install(lua, &library, host)?;
    song_url::install(lua, &library, host)?;
    love::install(lua, &library, host)?;
    mineral.set("library", library)
}

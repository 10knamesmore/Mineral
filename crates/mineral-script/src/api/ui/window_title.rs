//! TUI 本地窗口标题覆盖；nil 撤销并回落配置模板。

use mlua::{Lua, Table};

use crate::tui::{TuiCommand, TuiHost};

/// 安装 window_title 出口，不更新 daemon 配置或发送协议事件。
pub(crate) fn install(lua: &Lua, ui: &Table, host: &TuiHost) -> mlua::Result<()> {
    let collector = host.clone();
    ui.set(
        "window_title",
        lua.create_function(move |_lua, text: Option<String>| {
            collector.push(TuiCommand::WindowTitle { text });
            Ok(())
        })?,
    )
}

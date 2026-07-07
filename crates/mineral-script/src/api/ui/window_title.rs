//! `mineral.ui.window_title(text)`:窗口标题整串覆盖(脚本自渲染)。
//!
//! 渲染产物直通:不进配置合成、不触发落型校验,高频刷(如 10fps 动态标题)
//! 零成本。`nil` 撤销覆盖,client 回落结构化模板(配置 `tui.window_title`);
//! daemon 重启即清。

use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `window_title` 挂到 `ui` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `ui`: `mineral.ui` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, ui: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    ui.set(
        "window_title",
        lua.create_function(move |_lua, text: Option<String>| {
            // 接收端关闭(daemon 停机)时静默丢,脚本不感知。
            let _ = commands.send(ScriptCmd::WindowTitle { text });
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use crate::api::test_support::{drain_cmds, vm_with_commands};
    use crate::message::ScriptCmd;

    #[test]
    fn window_title_sends_text_and_nil_revokes() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.ui.window_title("⏸ 歌名")"#).exec()?;
        lua.load("mineral.ui.window_title(nil)").exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![
                ScriptCmd::WindowTitle {
                    text: Some("⏸ 歌名".to_owned()),
                },
                ScriptCmd::WindowTitle { text: None },
            ]
        );
        Ok(())
    }

    #[test]
    fn window_title_rejects_table_value() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua.load("mineral.ui.window_title({})").exec();
        assert!(result.is_err(), "非字符串值必须报 Lua 错");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }
}

//! `mineral.config.override(path, value)`:session 级配置覆盖。
//!
//! `path` 必须是**真实配置路径**(如 `tui.lyrics.fullscreen_line_gap`);
//! daemon 把它深合并进有效配置并落型校验(坏路径 / 坏值被剔除 + 警告),
//! 结果经 ConfigChanged 推给订阅 client。`value = nil` 撤销覆盖,回落
//! 配置文件的值;不碰配置文件本身,daemon 重启即清。

use mlua::{Lua, Table};

use crate::api::value::lua_to_bus;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `override` 挂到 `config` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `config`: `mineral.config` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, config: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    config.set(
        "override",
        lua.create_function(move |_lua, (path, value): (String, mlua::Value)| {
            // nil 收敛成「撤销」:`Some(Nil)` 不上 wire,避免「覆盖成 Nil」
            // 与「撤销」两义。
            let value = match value {
                mlua::Value::Nil => None,
                other => Some(lua_to_bus(&other, /*depth*/ 0)?),
            };
            // 接收端关闭(daemon 停机)时静默丢,脚本不感知。
            let _ = commands.send(ScriptCmd::ConfigOverride { path, value });
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use mineral_protocol::BusValue;

    use crate::api::test_support::{drain_cmds, vm_with_commands};
    use crate::message::ScriptCmd;

    #[test]
    fn override_sends_cmd_with_value() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.config.override("tui.lyrics.fullscreen_line_gap", 2)"#)
            .exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![ScriptCmd::ConfigOverride {
                path: "tui.lyrics.fullscreen_line_gap".to_owned(),
                value: Some(BusValue::Int(2)),
            }]
        );
        Ok(())
    }

    #[test]
    fn override_nil_converges_to_revoke() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.config.override("tui.lyrics.fullscreen_line_gap", nil)"#)
            .exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![ScriptCmd::ConfigOverride {
                path: "tui.lyrics.fullscreen_line_gap".to_owned(),
                value: None,
            }],
            "nil 必须收敛成撤销(None),不得发 Some(Nil)"
        );
        Ok(())
    }

    #[test]
    fn override_rejects_function_value() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua
            .load(r#"mineral.config.override("k", function() end)"#)
            .exec();
        assert!(result.is_err(), "function 值必须报 Lua 错");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }
}

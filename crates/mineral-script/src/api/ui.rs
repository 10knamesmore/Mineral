//! `mineral.ui.toast` 与 `mineral.log.{info,warn}`:脚本对用户 / 日志的
//! 两条出声通道。

use mineral_protocol::{Event, ToastKind};
use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 把 `ui` / `log` 两张子表挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(toast 闭包捕获其 push 出口)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let ui = lua.create_table()?;
    let push = host.push.clone();
    ui.set(
        "toast",
        lua.create_function(move |_lua, (msg, opts): (String, Option<Table>)| {
            let (kind, id) = parse_toast_opts(opts.as_ref())?;
            // 接收端关闭(daemon 停机)时静默丢,脚本不感知。
            let _ = push.send(Event::Toast {
                kind,
                content: msg,
                id,
            });
            Ok(())
        })?,
    )?;
    mineral.set("ui", ui)?;

    let log = lua.create_table()?;
    log.set(
        "info",
        lua.create_function(|_lua, msg: String| {
            mineral_log::info!(target: "script", "{msg}");
            Ok(())
        })?,
    )?;
    log.set(
        "warn",
        lua.create_function(|_lua, msg: String| {
            mineral_log::warn!(target: "script", "{msg}");
            Ok(())
        })?,
    )?;
    mineral.set("log", log)
}

/// 解析 toast 的可选 opts 表:`{ kind?: "info"|"warn"|"error", id?: string }`。
///
/// # Params:
///   - `opts`: Lua 侧第二个实参(省略为 `None`)
///
/// # Return:
///   `(kind, id)`;kind 缺省 `Info`,未知 kind 名报 Lua 错(不静默降级)。
fn parse_toast_opts(opts: Option<&Table>) -> mlua::Result<(ToastKind, Option<String>)> {
    let Some(opts) = opts else {
        return Ok((ToastKind::Info, None));
    };
    let kind = match opts.get::<Option<String>>("kind")?.as_deref() {
        None | Some("info") => ToastKind::Info,
        Some("warn") => ToastKind::Warn,
        Some("error") => ToastKind::Error,
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "unknown toast kind {other:?}, expected \"info\" | \"warn\" | \"error\""
            )));
        }
    };
    let id = opts.get::<Option<String>>("id")?;
    Ok((kind, id))
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{Event, ToastKind};
    use mlua::Lua;
    use tokio::sync::mpsc::unbounded_channel;

    use crate::host::{ScriptHost, install_api};

    /// 装好 API 的 VM + push 接收端。
    fn vm_with_push() -> color_eyre::Result<(Lua, tokio::sync::mpsc::UnboundedReceiver<Event>)> {
        let (cmd_tx, _cmd_rx) = unbounded_channel();
        let (push_tx, push_rx) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        Ok((lua, push_rx))
    }

    #[test]
    fn toast_with_opts_reaches_push_sink() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(r#"mineral.ui.toast("hello", { kind = "warn", id = "greet" })"#)
            .exec()?;
        let event = push_rx.try_recv()?;
        assert_eq!(
            event,
            Event::Toast {
                kind: ToastKind::Warn,
                content: "hello".to_owned(),
                id: Some("greet".to_owned()),
            }
        );
        Ok(())
    }

    #[test]
    fn toast_defaults_to_info_without_opts() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(r#"mineral.ui.toast("plain")"#).exec()?;
        let event = push_rx.try_recv()?;
        assert_eq!(
            event,
            Event::Toast {
                kind: ToastKind::Info,
                content: "plain".to_owned(),
                id: None,
            }
        );
        Ok(())
    }

    #[test]
    fn unknown_toast_kind_is_lua_error() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        let result = lua
            .load(r#"mineral.ui.toast("x", { kind = "fatal" })"#)
            .exec();
        assert!(result.is_err(), "未知 kind 必须报 Lua 错,不静默降级");
        assert!(push_rx.try_recv().is_err(), "报错时不得发出 toast");
        Ok(())
    }

    #[test]
    fn log_calls_do_not_error() -> color_eyre::Result<()> {
        let (lua, _push_rx) = vm_with_push()?;
        lua.load(r#"mineral.log.info("i"); mineral.log.warn("w")"#)
            .exec()?;
        Ok(())
    }
}

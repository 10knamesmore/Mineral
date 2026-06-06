//! `mineral.ui.toast(msg, opts)`:推送 toast 到 client(同 id 顶替不堆叠)。

use mineral_protocol::{Event, ToastKind};
use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// `toast` 的已解析 opts(Lua 表 → 强类型边界)。
struct ToastOpts {
    /// 视觉级别(缺省 `Info`)。
    kind: ToastKind,

    /// 顶替键(同 id 替换不堆叠;缺省不参与顶替)。
    id: Option<String>,

    /// 展示秒数(缺省用 client 配置 `toast.flash_ttl_secs`)。
    ttl_secs: Option<u64>,
}

/// 把 `toast` 挂到 `ui` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `ui`: `mineral.ui` 子表
///   - `host`: 宿主句柄(闭包捕获其推送出口)
pub(crate) fn install(lua: &Lua, ui: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let push = host.push.clone();
    ui.set(
        "toast",
        lua.create_function(move |_lua, (msg, opts): (String, Option<Table>)| {
            let opts = parse_opts(opts.as_ref())?;
            // 接收端关闭(daemon 停机)时静默丢,脚本不感知。
            let _ = push.send(Event::Toast {
                kind: opts.kind,
                content: msg,
                id: opts.id,
                ttl_secs: opts.ttl_secs,
            });
            Ok(())
        })?,
    )
}

/// 解析 opts 表:`{ kind?: "info"|"warn"|"error", id?: string, ttl_secs?: integer }`。
///
/// # Params:
///   - `opts`: Lua 侧第二个实参(省略为 `None`)
///
/// # Return:
///   解析后的 [`ToastOpts`];未知 kind 名 / 负 ttl 报 Lua 错(不静默降级)。
fn parse_opts(opts: Option<&Table>) -> mlua::Result<ToastOpts> {
    let Some(opts) = opts else {
        return Ok(ToastOpts {
            kind: ToastKind::Info,
            id: None,
            ttl_secs: None,
        });
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
    let ttl_secs = opts
        .get::<Option<i64>>("ttl_secs")?
        .map(|raw| {
            u64::try_from(raw).map_err(|_negative| {
                mlua::Error::RuntimeError(format!("toast ttl_secs must be >= 0, got {raw}"))
            })
        })
        .transpose()?;
    Ok(ToastOpts { kind, id, ttl_secs })
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{Event, ToastKind};

    use crate::api::test_support::vm_with_push;

    #[test]
    fn toast_with_opts_reaches_push_sink() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(r#"mineral.ui.toast("hello", { kind = "warn", id = "greet", ttl_secs = 10 })"#)
            .exec()?;
        let event = push_rx.try_recv()?;
        assert_eq!(
            event,
            Event::Toast {
                kind: ToastKind::Warn,
                content: "hello".to_owned(),
                id: Some("greet".to_owned()),
                ttl_secs: Some(10),
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
                ttl_secs: None,
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
    fn toast_invalid_ttl_is_lua_error() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        let result = lua
            .load(r#"mineral.ui.toast("x", { ttl_secs = -1 })"#)
            .exec();
        assert!(result.is_err(), "负 ttl 必须报 Lua 错");
        assert!(push_rx.try_recv().is_err(), "报错时不得发出 toast");
        Ok(())
    }
}

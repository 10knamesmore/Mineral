//! `mineral.ui.toast(msg, opts)`:推送单行 toast 到 client(同 id 顶替不堆叠)。
//! msg 接受任意值(tostring)或 span 数组(行内样式,与 `ui.card` 同一词表)。

use mineral_protocol::{Event, TextSpan, ToastKind};
use mlua::{Lua, Table};

use crate::api::ui::span::parse_line;
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
        lua.create_function(move |_lua, (msg, opts): (mlua::Value, Option<Table>)| {
            // `print` 式宽容:任意值经 tostring 显示;nil 静默跳过——
            // `toast(ctx.search_query)` 这类可空链无词时安静,不炸回调。
            // 表按 span 数组解析(行内样式,与 card body 的一行同形)。
            let content = match msg {
                mlua::Value::Nil => return Ok(()),
                mlua::Value::Table(line) => parse_line(&line)?,
                other => vec![TextSpan::plain(other.to_string()?)],
            };
            let opts = parse_opts(opts.as_ref())?;
            // 接收端关闭(daemon 停机)时静默丢,脚本不感知。
            let _ = push.send(Event::Toast {
                kind: opts.kind,
                content,
                id: opts.id,
                ttl_secs: opts.ttl_secs,
            });
            Ok(())
        })?,
    )
}

/// 解析 kind 名:`"info"` / `"warn"` / `"error"`(缺省 `Info`)。
/// toast 与 card 共用这一词表。
///
/// # Params:
///   - `name`: Lua 侧 `kind` 字段值(省略为 `None`)
///
/// # Return:
///   对应级别;未知名报 Lua 错(不静默降级)。
pub(super) fn parse_kind(name: Option<&str>) -> mlua::Result<ToastKind> {
    match name {
        None | Some("info") => Ok(ToastKind::Info),
        Some("warn") => Ok(ToastKind::Warn),
        Some("error") => Ok(ToastKind::Error),
        Some(other) => Err(mlua::Error::RuntimeError(format!(
            "unknown kind {other:?}, expected \"info\" | \"warn\" | \"error\""
        ))),
    }
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
    let kind = parse_kind(opts.get::<Option<String>>("kind")?.as_deref())?;
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
    use mineral_protocol::{Event, SpanFg, TextSpan, ToastKind};

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
                content: vec![TextSpan::plain("hello")],
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
                content: vec![TextSpan::plain("plain")],
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
    fn toast_accepts_any_value_and_skips_nil() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(
            r#"
            mineral.ui.toast(nil)          -- nil 静默跳过,不报错不显示
            mineral.ui.toast(42)           -- 数字经 tostring
            mineral.ui.toast(true)         -- boolean 经 tostring
            "#,
        )
        .exec()?;
        let first = push_rx.try_recv()?;
        assert!(
            matches!(first, Event::Toast { ref content, .. } if *content == vec![TextSpan::plain("42")]),
            "nil 跳过后第一条应是 42,实得 {first:?}"
        );
        let second = push_rx.try_recv()?;
        assert!(
            matches!(second, Event::Toast { ref content, .. } if *content == vec![TextSpan::plain("true")]),
            "实得 {second:?}"
        );
        assert!(push_rx.try_recv().is_err(), "nil 不该产生事件");
        Ok(())
    }

    #[test]
    fn toast_accepts_span_array_msg() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(r#"mineral.ui.toast({ "音量 ", { "42", fg = "accent", bold = true } })"#)
            .exec()?;
        let event = push_rx.try_recv()?;
        assert_eq!(
            event,
            Event::Toast {
                kind: ToastKind::Info,
                content: vec![
                    TextSpan::plain("音量 "),
                    TextSpan {
                        text: "42".to_owned(),
                        fg: Some(SpanFg::Accent),
                        bold: true,
                        italic: false,
                        underline: false,
                        dim: false,
                        align: mineral_protocol::SpanAlign::Left,
                    },
                ],
                id: None,
                ttl_secs: None,
            },
            "span 数组 msg 应解析成行内样式"
        );
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

//! TUI 本地单行通知；文本或 span 数组共用通知解析词表。

use mineral_protocol::{TextSpan, ToastKind};
use mlua::{Lua, Table};

use crate::api::ui::span::parse_line;
use crate::tui::{TuiCommand, TuiHost};

/// 已解析的通知选项。
struct ToastOpts {
    /// 视觉级别，省略为 Info。
    kind: ToastKind,

    /// 同 id 顶替，省略时独立显示。
    id: Option<String>,

    /// 展示秒数，省略时使用 TUI 配置。
    ttl_secs: Option<u64>,
}

/// 安装 TUI 的 toast 出口；nil 跳过，其余标量按 tostring 显示。
pub(crate) fn install(lua: &Lua, ui: &Table, host: &TuiHost) -> mlua::Result<()> {
    let collector = host.clone();
    ui.set(
        "toast",
        lua.create_function(move |_lua, (msg, opts): (mlua::Value, Option<Table>)| {
            let content = match msg {
                mlua::Value::Nil => return Ok(()),
                mlua::Value::Table(line) => parse_line(&line)?,
                other => vec![TextSpan::plain(other.to_string()?)],
            };
            let opts = parse_opts(opts.as_ref())?;
            collector.push(TuiCommand::Toast {
                kind: opts.kind,
                content,
                id: opts.id,
                ttl_secs: opts.ttl_secs,
            });
            Ok(())
        })?,
    )
}

/// toast 与 card 的通知级别共用 info / warn / error；未知值报 Lua 错。
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

/// 解析 opts；未知 kind、错误字段类型或负 ttl 报 Lua 错。
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

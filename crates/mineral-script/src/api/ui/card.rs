//! TUI 本地多行通知卡片；标题与正文共用 span 解析。

use mineral_protocol::TextSpan;
use mlua::{Lua, Table, Value};

use crate::api::ui::span::parse_line;
use crate::api::ui::toast::parse_kind;
use crate::tui::{TuiCommand, TuiHost};

/// 安装 card 出口；必须提供 body，未提供 ttl 的卡片驻留至用户关闭。
pub(crate) fn install(lua: &Lua, ui: &Table, host: &TuiHost) -> mlua::Result<()> {
    let collector = host.clone();
    ui.set(
        "card",
        lua.create_function(move |_lua, opts: Table| {
            let kind = parse_kind(opts.get::<Option<String>>("kind")?.as_deref())?;
            let id = opts.get::<Option<String>>("id")?;
            let title = parse_title(opts.get::<Value>("title")?)?;
            let ttl_secs = opts
                .get::<Option<i64>>("ttl_secs")?
                .map(|raw| {
                    u64::try_from(raw).map_err(|_negative| {
                        mlua::Error::RuntimeError(format!("card ttl_secs must be >= 0, got {raw}"))
                    })
                })
                .transpose()?;
            let body_table = opts.get::<Option<Table>>("body")?.ok_or_else(|| {
                mlua::Error::RuntimeError("card body is required (table of lines)".to_owned())
            })?;
            let body = parse_body(&body_table)?;
            collector.push(TuiCommand::Card {
                kind,
                id,
                title,
                body,
                ttl_secs,
            });
            Ok(())
        })?,
    )
}

/// nil 不显示标题；字符串与 span 数组均可作为标题，其余类型报错。
fn parse_title(title: Value) -> mlua::Result<Vec<TextSpan>> {
    match title {
        Value::Nil => Ok(Vec::new()),
        Value::String(s) => Ok(vec![TextSpan::plain(s.to_str()?.as_ref())]),
        Value::Table(line) => parse_line(&line),
        other => Err(mlua::Error::RuntimeError(format!(
            "card title must be a string or a table of spans, got {}",
            other.type_name()
        ))),
    }
}

/// body 的数组项是字符串行或 span 数组；字符串内的换行拆成多行。
fn parse_body(body: &Table) -> mlua::Result<Vec<Vec<TextSpan>>> {
    let mut lines = Vec::<Vec<TextSpan>>::new();
    for entry in body.sequence_values::<Value>() {
        match entry? {
            Value::String(s) => {
                let text = s.to_str()?;
                for line in text.split('\n') {
                    lines.push(vec![TextSpan::plain(line)]);
                }
            }
            Value::Table(spans) => lines.push(parse_line(&spans)?),
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "card body line must be a string or a table of spans, got {}",
                    other.type_name()
                )));
            }
        }
    }
    Ok(lines)
}

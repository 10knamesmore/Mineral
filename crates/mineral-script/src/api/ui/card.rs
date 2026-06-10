//! `mineral.ui.card(opts)`:推送驻留通知卡片到 client(多行 + 行内样式,
//! 用户显式关闭才退场;同 id 顶替不堆叠)。

use mineral_protocol::{Event, TextSpan};
use mlua::{Lua, Table, Value};

use crate::api::ui::span::{parse_line, parse_span};
use crate::api::ui::toast::parse_kind;
use crate::host::ScriptHost;

/// 把 `card` 挂到 `ui` 子表上。
///
/// Lua 形态(单表参数):
///
/// ```lua
/// mineral.ui.card {
///   title    = "更新要点",        -- 可省,画进卡片边框;也接受 span 数组
///   kind     = "warn",           -- info|warn|error,缺省 info
///   id       = "scrobble.fail",  -- 可省,同 id 顶替
///   ttl_secs = 8,                -- 可省;给了到时自动退场(边框随剩余时间变暗),缺省驻留(按键关闭)
///   body  = {
///     "纯文本行",                 -- 内嵌 \n 拆成多行
///     { "前缀 ", { "高亮", fg = "accent", bold = true } },
///   },
/// }
/// ```
///
/// # Params:
///   - `lua`: 目标 VM
///   - `ui`: `mineral.ui` 子表
///   - `host`: 宿主句柄(闭包捕获其推送出口)
pub(crate) fn install(lua: &Lua, ui: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let push = host.push.clone();
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
            // 接收端关闭(daemon 停机)时静默丢,脚本不感知。
            let _ = push.send(Event::Card {
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

/// 解析 title 字段:`nil`(不画)、字符串(纯文本)或 span 数组(行内样式)。
///
/// # Params:
///   - `title`: Lua 侧 `title` 字段值
///
/// # Return:
///   协议 spans(空 = 不画);非法类型报 Lua 错。
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

/// 解析 body 表:每个数组项是一行 —— 字符串(整行默认样式,内嵌 `\n` 拆成多行)
/// 或 span 数组(行内混排样式)。
///
/// # Params:
///   - `body`: Lua 侧 `body` 字段(数组表)
///
/// # Return:
///   协议结构的行 / spans;非法项报 Lua 错(不静默降级)。
fn parse_body(body: &Table) -> mlua::Result<Vec<Vec<TextSpan>>> {
    let mut lines = Vec::<Vec<TextSpan>>::new();
    for entry in body.sequence_values::<Value>() {
        match entry? {
            Value::String(s) => {
                let text = s.to_str()?;
                // 整段纯文本可能携带多行(如错误链),按 \n 展开成多张行。
                for line in text.split('\n') {
                    lines.push(vec![TextSpan::plain(line)]);
                }
            }
            Value::Table(spans) => {
                let mut line = Vec::<TextSpan>::new();
                for span in spans.sequence_values::<Value>() {
                    line.push(parse_span(span?)?);
                }
                lines.push(line);
            }
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

#[cfg(test)]
mod tests {
    use mineral_protocol::{Event, SpanAlign, SpanFg, TextSpan, ToastKind};

    use crate::api::test_support::vm_with_push;

    #[test]
    fn card_with_styled_spans_reaches_push_sink() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(
            r##"mineral.ui.card {
                title = "要点",
                kind = "warn",
                id = "release",
                body = {
                    "纯文本行",
                    { "前缀 ", { "高亮", fg = "accent", bold = true }, { "直给", fg = "#cc6600", align = "right" } },
                },
            }"##,
        )
        .exec()?;
        let event = push_rx.try_recv()?;
        assert_eq!(
            event,
            Event::Card {
                kind: ToastKind::Warn,
                id: Some("release".to_owned()),
                title: vec![TextSpan::plain("要点")],
                ttl_secs: None,
                body: vec![
                    vec![TextSpan::plain("纯文本行")],
                    vec![
                        TextSpan::plain("前缀 "),
                        TextSpan {
                            text: "高亮".to_owned(),
                            fg: Some(SpanFg::Accent),
                            bold: true,
                            italic: false,
                            underline: false,
                            dim: false,
                            align: SpanAlign::Left,
                        },
                        TextSpan {
                            text: "直给".to_owned(),
                            fg: Some(SpanFg::Rgb(0xcc, 0x66, 0x00)),
                            bold: false,
                            italic: false,
                            underline: false,
                            dim: false,
                            align: SpanAlign::Right,
                        },
                    ],
                ],
            }
        );
        Ok(())
    }

    #[test]
    fn card_defaults_and_multiline_string_splits() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(r#"mineral.ui.card { body = { "第一行\n第二行" } }"#)
            .exec()?;
        let event = push_rx.try_recv()?;
        assert_eq!(
            event,
            Event::Card {
                kind: ToastKind::Info,
                id: None,
                title: Vec::new(),
                ttl_secs: None,
                body: vec![
                    vec![TextSpan::plain("第一行")],
                    vec![TextSpan::plain("第二行")],
                ],
            },
            "缺省 kind=info、ttl 缺省驻留、字符串行内嵌 \\n 应拆行"
        );
        Ok(())
    }

    #[test]
    fn card_title_accepts_span_array() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(
            r#"mineral.ui.card { title = { "v1 ", { "要点", fg = "peach" } }, body = { "x" } }"#,
        )
        .exec()?;
        let event = push_rx.try_recv()?;
        let want_title = vec![
            TextSpan::plain("v1 "),
            TextSpan {
                text: "要点".to_owned(),
                fg: Some(SpanFg::Peach),
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                align: SpanAlign::Left,
            },
        ];
        assert!(
            matches!(event, Event::Card { ref title, .. } if *title == want_title),
            "title 应支持 span 数组,实得 {event:?}"
        );
        Ok(())
    }

    #[test]
    fn card_with_ttl_auto_expires_carries_secs() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        lua.load(r#"mineral.ui.card { ttl_secs = 8, body = { "x" } }"#)
            .exec()?;
        let event = push_rx.try_recv()?;
        assert!(
            matches!(
                event,
                Event::Card {
                    ttl_secs: Some(8),
                    ..
                }
            ),
            "ttl_secs 应原样上 wire,实得 {event:?}"
        );
        Ok(())
    }

    #[test]
    fn card_negative_ttl_is_lua_error() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        let result = lua
            .load(r#"mineral.ui.card { ttl_secs = -1, body = { "x" } }"#)
            .exec();
        assert!(result.is_err(), "负 ttl 必须报 Lua 错");
        assert!(push_rx.try_recv().is_err(), "报错时不得发出事件");
        Ok(())
    }

    #[test]
    fn card_without_body_is_lua_error() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        let result = lua.load(r#"mineral.ui.card { title = "孤标题" }"#).exec();
        assert!(result.is_err(), "缺 body 必须报 Lua 错");
        assert!(push_rx.try_recv().is_err(), "报错时不得发出事件");
        Ok(())
    }

    #[test]
    fn card_unknown_fg_is_lua_error() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        let result = lua
            .load(r#"mineral.ui.card { body = { { { "x", fg = "magenta" } } } }"#)
            .exec();
        assert!(result.is_err(), "未知 fg 必须报 Lua 错,不静默降级");
        assert!(push_rx.try_recv().is_err(), "报错时不得发出事件");
        Ok(())
    }

    #[test]
    fn card_span_without_text_is_lua_error() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        let result = lua
            .load(r#"mineral.ui.card { body = { { { fg = "red" } } } }"#)
            .exec();
        assert!(result.is_err(), "span 表缺位置 1 文本必须报 Lua 错");
        assert!(push_rx.try_recv().is_err(), "报错时不得发出事件");
        Ok(())
    }

    #[test]
    fn card_unknown_align_is_lua_error() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        let result = lua
            .load(r#"mineral.ui.card { body = { { { "x", align = "top" } } } }"#)
            .exec();
        assert!(result.is_err(), "未知 align 必须报 Lua 错");
        assert!(push_rx.try_recv().is_err(), "报错时不得发出事件");
        Ok(())
    }

    #[test]
    fn card_unknown_kind_is_lua_error() -> color_eyre::Result<()> {
        let (lua, mut push_rx) = vm_with_push()?;
        let result = lua
            .load(r#"mineral.ui.card { kind = "fatal", body = { "x" } }"#)
            .exec();
        assert!(result.is_err(), "未知 kind 必须报 Lua 错");
        assert!(push_rx.try_recv().is_err(), "报错时不得发出事件");
        Ok(())
    }
}

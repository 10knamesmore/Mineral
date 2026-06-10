//! 通知类 API(toast / card)共用的样式 span 解析:Lua 表 → 协议 [`TextSpan`]。

use mineral_protocol::{SpanAlign, SpanFg, TextSpan};
use mlua::{Table, Value};

/// 解析一行 spans(数组表,项为字符串或 span 表)。
///
/// # Params:
///   - `line`: Lua 侧的 span 数组表
///
/// # Return:
///   协议 spans;非法项报 Lua 错(不静默降级)。
pub(super) fn parse_line(line: &Table) -> mlua::Result<Vec<TextSpan>> {
    let mut spans = Vec::<TextSpan>::new();
    for span in line.sequence_values::<Value>() {
        spans.push(parse_span(span?)?);
    }
    Ok(spans)
}

/// 解析一个 span:字符串(纯文本)或
/// `{ text [1], fg?, bold?, italic?, underline?, dim?, align? }`。
///
/// # Params:
///   - `span`: span 数组里的一项
///
/// # Return:
///   协议 span;缺文本 / 未知 fg / 未知 align / 非法类型报 Lua 错。
pub(super) fn parse_span(span: Value) -> mlua::Result<TextSpan> {
    match span {
        Value::String(s) => Ok(TextSpan::plain(s.to_str()?.as_ref())),
        Value::Table(t) => {
            let text = t.get::<Option<String>>(1)?.ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "span table needs its text at position 1, e.g. { \"词\", fg = \"accent\" }"
                        .to_owned(),
                )
            })?;
            let fg = t
                .get::<Option<String>>("fg")?
                .map(|name| parse_fg(&name))
                .transpose()?;
            let align = parse_align(t.get::<Option<String>>("align")?.as_deref())?;
            Ok(TextSpan {
                text,
                fg,
                bold: t.get::<Option<bool>>("bold")?.unwrap_or(false),
                italic: t.get::<Option<bool>>("italic")?.unwrap_or(false),
                underline: t.get::<Option<bool>>("underline")?.unwrap_or(false),
                dim: t.get::<Option<bool>>("dim")?.unwrap_or(false),
                align,
            })
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "span must be a string or a table, got {}",
            other.type_name()
        ))),
    }
}

/// 解析行内段位名:`"left"` / `"center"` / `"right"`(缺省靠左)。
///
/// # Params:
///   - `name`: Lua 侧 `align` 字段值
///
/// # Return:
///   对应段位;未知名报 Lua 错。
fn parse_align(name: Option<&str>) -> mlua::Result<SpanAlign> {
    match name {
        None | Some("left") => Ok(SpanAlign::Left),
        Some("center") => Ok(SpanAlign::Center),
        Some("right") => Ok(SpanAlign::Right),
        Some(other) => Err(mlua::Error::RuntimeError(format!(
            "unknown align {other:?}, expected \"left\" | \"center\" | \"right\""
        ))),
    }
}

/// 解析 fg 名:主题角色(`text` / `subtext` / `overlay` / `accent` / `red` /
/// `yellow` / `green` / `peach`)或 `#rrggbb` 直给 RGB。
///
/// # Params:
///   - `name`: Lua 侧 `fg` 字段值
///
/// # Return:
///   对应的 [`SpanFg`];未知名报 Lua 错。
fn parse_fg(name: &str) -> mlua::Result<SpanFg> {
    match name {
        "text" => Ok(SpanFg::Text),
        "subtext" => Ok(SpanFg::Subtext),
        "overlay" => Ok(SpanFg::Overlay),
        "accent" => Ok(SpanFg::Accent),
        "red" => Ok(SpanFg::Red),
        "yellow" => Ok(SpanFg::Yellow),
        "green" => Ok(SpanFg::Green),
        "peach" => Ok(SpanFg::Peach),
        other => parse_hex(other).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "unknown fg {other:?}, expected a theme role (text/subtext/overlay/accent/\
                 red/yellow/green/peach) or \"#rrggbb\""
            ))
        }),
    }
}

/// 解析 `#rrggbb` 十六进制色;形状不符为 `None`。
fn parse_hex(s: &str) -> Option<SpanFg> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
    let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
    let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
    Some(SpanFg::Rgb(r, g, b))
}

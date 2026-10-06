//! 渲染本地 TUI 配置校验摘要;不读取 daemon 配置。

use super::TuiConfig;
use mineral_config::ConfigWarning;

/// 渲染当前 TUI 的配置摘要与告警,不读取任何 daemon 配置。
pub fn render_check(config: &TuiConfig, warnings: &[ConfigWarning], color: bool) -> String {
    let mut lines = vec![paint("Mineral TUI configuration check", "1;36", color)];
    lines.push(format!(
        "  cover cache (bytes): {}",
        config.cover().cache().disk()
    ));
    lines.push(format!("  heartbeat (s): {}", config.heartbeat_secs()));
    lines.push(format!(
        "  copy templates: {}",
        config.copy().templates().len()
    ));
    append_warnings(&mut lines, warnings, color);
    lines.join("\n")
}

/// 追加结构化诊断的摘要;底层错误保留在日志的 source 链中。
fn append_warnings(lines: &mut Vec<String>, warnings: &[ConfigWarning], color: bool) {
    if warnings.is_empty() {
        lines.push(paint("Config OK, no warnings.", "32", color));
    } else {
        lines.push(paint(
            &format!("{} warning(s):", warnings.len()),
            "1;31",
            color,
        ));
        for warning in warnings {
            lines.push(paint(&format!("  - {warning}"), "31", color));
        }
    }
}

/// 按需给文本加 ANSI 颜色;`color = false` 时原样返回。
///
/// # Params:
///   - `text`: 文本
///   - `code`: ANSI SGR 参数(如 `"32"`)
///   - `color`: 是否上色
///
/// # Return:
///   带 / 不带 ANSI 的文本
fn paint(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

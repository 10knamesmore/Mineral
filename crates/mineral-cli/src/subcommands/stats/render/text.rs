//! `stats` 子命令的人读文本渲染(comfy-table 表格 + 分级配色,风格同 `cache` 命令)。
//!
//! 渲染纯函数化(输入全显式给定),便于快照测试。颜色经 `color` 开关控制:`false`
//! (非 tty / 测试)强制无 ANSI,`true`(tty)启用上色。

use std::path::Path;

use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use mineral_stats::{
    EventCount, FinishReason, NamedEntry, PlayTail, Slice, StatsReport, StatusReport,
};

/// 分布条形格数。
const BAR_WIDTH: usize = 20;

/// stats.db 不存在时的友好提示(指向配置,不报错栈)。
pub fn render_absent() -> String {
    "stats.db does not exist yet — nothing recorded, or stats.level = \"off\".\n\
     To enable: set stats.level = \"core\" (plays + sessions) or \"full\" (all interactions) in config.lua."
        .to_owned()
}

/// 收听 ms → `"1h30m"`(不足 1 小时省时段仅出分)。
fn fmt_listen(ms: i64) -> String {
    let hours = ms / 3_600_000;
    let mins = (ms % 3_600_000) / 60_000;
    if hours > 0 {
        format!("{hours}h{mins}m")
    } else {
        format!("{mins}m")
    }
}

/// 榜项展示名:回查命中用名,否则回落 qualified id。
fn display_name(entry: &NamedEntry) -> &str {
    entry.name.as_deref().unwrap_or(&entry.id)
}

/// 一条 unicode 条形(按 `value/max` 比例填 `█`)。空 max 出空条。
fn bar(value: i64, max: i64, width: usize) -> String {
    if max <= 0 {
        return String::new();
    }
    let width_i = i64::try_from(width).unwrap_or(0);
    let filled = usize::try_from(value.saturating_mul(width_i) / max)
        .unwrap_or(0)
        .min(width);
    "█".repeat(filled)
}

/// 建带圆角边框、关闭动态排版(列宽随内容、不换行,保证确定性)的基底表。
fn base_table(color: bool) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Disabled);
    if color {
        table.enforce_styling();
    } else {
        table.force_no_tty();
    }
    table
}

/// 表头单元格:加粗。
fn head_cell(text: &str, color: bool) -> Cell {
    let cell = Cell::new(text);
    if color {
        cell.add_attribute(Attribute::Bold)
    } else {
        cell
    }
}

/// 行首标签单元格:加粗 + 青色。
fn label_cell(text: &str, color: bool) -> Cell {
    let cell = Cell::new(text);
    if color {
        cell.add_attribute(Attribute::Bold).fg(Color::Cyan)
    } else {
        cell
    }
}

/// 条件上色:`color` 为真才给单元格上前景色。
fn maybe_fg(cell: Cell, color: bool, fg: Color) -> Cell {
    if color { cell.fg(fg) } else { cell }
}

/// 给一段表格文本加 `▸ 标题` 前缀行。
fn labeled(title: &str, body: &str) -> String {
    format!("▸ {title}\n{body}")
}

/// 结束原因的落库串(与 stats.db 一致)+ 语义配色:完播绿、跳过黄、停止灰、失败红。
fn finish_cell(reason: FinishReason, color: bool) -> Cell {
    let (text, fg) = match reason {
        FinishReason::Eof => ("eof", Color::Green),
        FinishReason::Skip => ("skip", Color::Yellow),
        FinishReason::Stop => ("stop", Color::DarkGrey),
        FinishReason::Error => ("error", Color::Red),
    };
    maybe_fg(Cell::new(text), color, fg)
}

/// 渲染埋点系统自身状态(kv 表:路径 / 大小 / 档位 / 覆盖窗 / 三项计数)。
///
/// # Params:
///   - `path`: stats.db 路径
///   - `size_bytes`: db 文件字节数
///   - `level`: 当前采集档位串(off / core / full)
///   - `report`: 状态聚合
///   - `color`: 是否上色(非 tty 传 `false`)
///
/// # Return:
///   状态表文本
pub fn render_status(
    path: &Path,
    size_bytes: u64,
    level: &str,
    report: &StatusReport,
    color: bool,
) -> String {
    let coverage = match (report.first_play_at, report.last_play_at) {
        (Some(first), Some(last)) => format!("{first} → {last}(epoch ms)"),
        _ => "(no plays)".to_owned(),
    };
    let level_fg = match level {
        "full" => Color::Green,
        "core" => Color::Yellow,
        _ => Color::DarkGrey,
    };
    let mut table = base_table(color);
    table.set_header(vec![head_cell("field", color), head_cell("value", color)]);
    table.add_row(vec![
        label_cell("stats.db", color),
        Cell::new(path.display().to_string()),
    ]);
    table.add_row(vec![
        label_cell("size", color),
        Cell::new(format!("{size_bytes} bytes")),
    ]);
    table.add_row(vec![
        label_cell("level", color),
        maybe_fg(Cell::new(level), color, level_fg),
    ]);
    table.add_row(vec![label_cell("coverage", color), Cell::new(coverage)]);
    table.add_row(vec![
        label_cell("plays / sessions / events", color),
        Cell::new(format!(
            "{} / {} / {}",
            report.plays, report.sessions, report.events
        )),
    ]);
    table.to_string()
}

/// 渲染最近播放流水 tail(表:起播 ms / 歌曲 / 收听 / 结束原因)。
pub fn render_history(plays: &[PlayTail], color: bool) -> String {
    if plays.is_empty() {
        return "(no plays)".to_owned();
    }
    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("started (ms)", color),
        head_cell("song", color),
        head_cell("listened (ms)", color),
        head_cell("finish", color),
    ]);
    for p in plays {
        table.add_row(vec![
            Cell::new(p.started_at),
            Cell::new(p.song.qualified()),
            Cell::new(p.listen_ms),
            finish_cell(p.finish_reason, color),
        ]);
    }
    table.to_string()
}

/// 渲染一张 top 榜(表:排名 / 名称 / 次数 / 收听);`header` 是榜标题(不带尾冒号)。
pub fn render_top(entries: &[NamedEntry], header: &str, color: bool) -> String {
    if entries.is_empty() {
        return format!("▸ {header}\n  (none)");
    }
    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("#", color),
        head_cell("name", color),
        head_cell("plays", color),
        head_cell("listened", color),
    ]);
    for (i, e) in entries.iter().enumerate() {
        table.add_row(vec![
            Cell::new(i + 1),
            Cell::new(display_name(e)),
            maybe_fg(Cell::new(e.plays), color, Color::Green),
            Cell::new(fmt_listen(e.listen_ms)),
        ]);
    }
    labeled(header, &table.to_string())
}

/// 渲染各事件表行数(表:事件表 / 行数,0 行也列出)。
fn events_table(counts: &[EventCount], color: bool) -> String {
    if counts.is_empty() {
        return "  (none)".to_owned();
    }
    let mut table = base_table(color);
    table.set_header(vec![head_cell("table", color), head_cell("rows", color)]);
    for c in counts {
        table.add_row(vec![Cell::new(&c.table), Cell::new(c.count)]);
    }
    table.to_string()
}

/// 渲染一组「值 → 次数」分布(表:值 / 次数 / 占比条;空值显示占位)。
fn slices_table(slices: &[Slice], color: bool) -> String {
    if slices.is_empty() {
        return "  (none)".to_owned();
    }
    let max = slices.iter().map(|s| s.plays).max().unwrap_or(0);
    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("value", color),
        head_cell("plays", color),
        head_cell("distribution", color),
    ]);
    for s in slices {
        let value = if s.value.is_empty() {
            "(unknown)"
        } else {
            &s.value
        };
        table.add_row(vec![
            Cell::new(value),
            maybe_fg(Cell::new(s.plays), color, Color::Green),
            maybe_fg(Cell::new(bar(s.plays, max, BAR_WIDTH)), color, Color::Cyan),
        ]);
    }
    table.to_string()
}

/// 渲染一屏盘点报告:窗口 + 总量表 + top 歌 / 专辑 / 艺人 + 来源分布 + 事件量。
///
/// # Params:
///   - `report`: 已落名的完整报告
///   - `window`: 窗口标签("2026" / "all" 等)
///   - `color`: 是否上色
///
/// # Return:
///   多块报告文本
pub fn render_report(report: &StatsReport, window: &str, color: bool) -> String {
    let t = &report.totals;
    let e = &report.endurance;
    let mut totals_table = base_table(color);
    totals_table.set_header(vec![
        head_cell("overview", color),
        head_cell("value", color),
    ]);
    totals_table.add_row(vec![label_cell("window", color), Cell::new(window)]);
    totals_table.add_row(vec![
        label_cell("plays / completed / skipped", color),
        Cell::new(format!("{} / {} / {}", t.plays, t.completed, t.skipped)),
    ]);
    totals_table.add_row(vec![
        label_cell("listening time", color),
        maybe_fg(Cell::new(fmt_listen(t.listen_ms)), color, Color::Green),
    ]);
    totals_table.add_row(vec![
        label_cell("songs / active days / discoveries", color),
        Cell::new(format!(
            "{} / {} / {}",
            t.distinct_songs,
            t.active_days,
            report.discoveries.new_songs.len()
        )),
    ]);
    totals_table.add_row(vec![
        label_cell("sessions / longest / streak", color),
        Cell::new(format!(
            "{} / {} / {}d",
            e.sessions,
            fmt_listen(e.longest_ms),
            e.streak_days
        )),
    ]);

    let mut blocks = vec![totals_table.to_string()];
    for (title, entries) in [
        ("top songs", &report.top_songs),
        ("top albums", &report.top_albums),
        ("top artists", &report.top_artists),
    ] {
        if !entries.is_empty() {
            blocks.push(render_top(entries, title, color));
        }
    }
    blocks.push(labeled(
        "by source",
        &slices_table(&report.distributions.by_source, color),
    ));
    blocks.push(labeled(
        "events",
        &events_table(&report.events.table_counts, color),
    ));
    blocks.join("\n\n")
}

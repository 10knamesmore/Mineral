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
    "stats.db 尚不存在——从未采集,或 stats.level = \"off\"。\n\
     开启采集:在 config.lua 里设 stats.level = \"core\"(播放 + 会话)或 \"full\"(全谱交互)。"
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
        _ => "(无播放记录)".to_owned(),
    };
    let level_fg = match level {
        "full" => Color::Green,
        "core" => Color::Yellow,
        _ => Color::DarkGrey,
    };
    let mut table = base_table(color);
    table.set_header(vec![head_cell("字段", color), head_cell("值", color)]);
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
        return "(无播放记录)".to_owned();
    }
    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("起播(ms)", color),
        head_cell("歌曲", color),
        head_cell("收听(ms)", color),
        head_cell("结束", color),
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
        return format!("▸ {header}\n  (无)");
    }
    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("#", color),
        head_cell("名称", color),
        head_cell("次数", color),
        head_cell("收听", color),
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
        return "  (无)".to_owned();
    }
    let mut table = base_table(color);
    table.set_header(vec![head_cell("事件表", color), head_cell("行数", color)]);
    for c in counts {
        table.add_row(vec![Cell::new(&c.table), Cell::new(c.count)]);
    }
    table.to_string()
}

/// 渲染一组「值 → 次数」分布(表:值 / 次数 / 占比条;空值显示占位)。
fn slices_table(slices: &[Slice], color: bool) -> String {
    if slices.is_empty() {
        return "  (无)".to_owned();
    }
    let max = slices.iter().map(|s| s.plays).max().unwrap_or(0);
    let mut table = base_table(color);
    table.set_header(vec![
        head_cell("值", color),
        head_cell("次数", color),
        head_cell("分布", color),
    ]);
    for s in slices {
        let value = if s.value.is_empty() {
            "(未知)"
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
    totals_table.set_header(vec![head_cell("总览", color), head_cell("值", color)]);
    totals_table.add_row(vec![label_cell("窗口", color), Cell::new(window)]);
    totals_table.add_row(vec![
        label_cell("播放 / 完播 / 跳过", color),
        Cell::new(format!("{} / {} / {}", t.plays, t.completed, t.skipped)),
    ]);
    totals_table.add_row(vec![
        label_cell("收听时长", color),
        maybe_fg(Cell::new(fmt_listen(t.listen_ms)), color, Color::Green),
    ]);
    totals_table.add_row(vec![
        label_cell("涉及歌曲 / 活跃天数 / 新发现", color),
        Cell::new(format!(
            "{} / {} / {}",
            t.distinct_songs,
            t.active_days,
            report.discoveries.new_songs.len()
        )),
    ]);
    totals_table.add_row(vec![
        label_cell("会话 / 最长会话 / 连续天数", color),
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

#[cfg(test)]
mod tests {
    use super::{render_absent, render_history, render_report, render_status, render_top};
    use mineral_model::{AlbumId, ArtistId, SongId, SourceKind};
    use mineral_stats::{
        Discoveries, Distributions, Endurance, EventCount, EventSummary, FinishReason, NamedEntry,
        PlayTail, RawReport, Slice, StatsReport, StatusReport, TopAlbum, TopArtist, TopSong,
        Totals, combine,
    };

    fn named(id: &str, name: Option<&str>, plays: i64, listen_ms: i64) -> NamedEntry {
        NamedEntry {
            id: id.to_owned(),
            name: name.map(str::to_owned),
            plays,
            listen_ms,
        }
    }

    /// 造一份带名报告(经 `combine`,因 `StatsReport` 非穷尽不能字面量构造)。
    fn sample_report() -> StatsReport {
        let raw = RawReport {
            totals: Totals {
                listen_ms: 3_600_000 + 30 * 60_000,
                plays: 12,
                completed: 9,
                skipped: 3,
                distinct_songs: 7,
                active_days: 4,
            },
            top_songs: vec![TopSong {
                song: SongId::new(SourceKind::NETEASE, "1"),
                name: Some("稻香".to_owned()),
                plays: 9,
                listen_ms: 540_000,
            }],
            top_albums: vec![TopAlbum {
                album: AlbumId::new(SourceKind::NETEASE, "al"),
                name: Some("魔杰座".to_owned()),
                plays: 6,
                listen_ms: 360_000,
            }],
            top_artists: vec![TopArtist {
                artist: ArtistId::new(SourceKind::NETEASE, "jay"),
                name: Some("周杰伦".to_owned()),
                plays: 9,
                listen_ms: 540_000,
            }],
            distributions: Distributions {
                by_source: vec![
                    Slice {
                        value: "netease".to_owned(),
                        plays: 8,
                    },
                    Slice {
                        value: String::new(),
                        plays: 1,
                    },
                ],
                ..Distributions::default()
            },
            hourly: Vec::new(),
            discoveries: Discoveries {
                new_songs: vec![
                    SongId::new(SourceKind::NETEASE, "1"),
                    SongId::new(SourceKind::NETEASE, "2"),
                ],
                first_play: None,
                last_play: None,
            },
            endurance: Endurance {
                sessions: 2,
                avg_ms: 600_000,
                longest_ms: 1_200_000,
                streak_days: 3,
            },
            events: EventSummary {
                table_counts: vec![EventCount {
                    table: "searches".to_owned(),
                    count: 12,
                }],
                ..EventSummary::default()
            },
        };
        combine(raw)
    }

    #[test]
    fn status_shows_counts_level_and_coverage() {
        let report = StatusReport {
            plays: 3,
            sessions: 1,
            events: 2,
            first_play_at: Some(1000),
            last_play_at: Some(5000),
        };
        let out = render_status(
            std::path::Path::new("/x/stats.db"),
            4096,
            "full",
            &report,
            /*color*/ false,
        );
        assert!(out.contains('3'), "{out}");
        assert!(out.contains("full"), "{out}");
        assert!(out.contains("1000 → 5000"), "{out}");
    }

    #[test]
    fn absent_points_to_config() {
        assert!(render_absent().contains("stats.level"));
    }

    #[test]
    fn history_lists_qualified_ids_and_reasons() {
        let plays = vec![
            PlayTail {
                song: SongId::new(SourceKind::NETEASE, "1"),
                started_at: 1000,
                listen_ms: 3000,
                finish_reason: FinishReason::Eof,
            },
            PlayTail {
                song: SongId::new(SourceKind::BILIBILI, "BV2"),
                started_at: 500,
                listen_ms: 200,
                finish_reason: FinishReason::Skip,
            },
        ];
        let out = render_history(&plays, /*color*/ false);
        assert!(out.contains("netease:1"), "{out}");
        assert!(out.contains("eof"), "{out}");
        assert!(out.contains("bilibili:BV2"), "{out}");
        assert!(out.contains("skip"), "{out}");
    }

    #[test]
    fn history_empty_message() {
        assert_eq!(render_history(&[], /*color*/ false), "(无播放记录)");
    }

    /// top 榜用回查名,缺名回落 qualified id。
    #[test]
    fn top_uses_name_or_falls_back_to_id() {
        let entries = vec![
            named("netease:1", Some("稻香"), 9, 540_000),
            named("bilibili:BV2", None, 3, 180_000),
        ];
        let out = render_top(&entries, "top songs", /*color*/ false);
        assert!(out.contains("稻香"), "命中名:{out}");
        assert!(out.contains("bilibili:BV2"), "缺名回落 id:{out}");
        assert!(out.contains('9'), "{out}");
    }

    #[test]
    fn top_empty_message() {
        assert!(render_top(&[], "top songs", /*color*/ false).contains("(无)"));
    }

    #[test]
    fn report_shows_window_totals_and_named_tops() {
        let report = sample_report();
        let out = render_report(&report, "2026", /*color*/ false);
        assert!(out.contains("2026"), "{out}");
        assert!(out.contains("12"), "{out}");
        assert!(out.contains("1h30m"), "{out}");
        assert!(out.contains('2'), "discoveries:{out}");
        assert!(out.contains("3d"), "{out}");
        assert!(out.contains("稻香"), "top song 带名:{out}");
        assert!(out.contains("魔杰座"), "top album 带名:{out}");
        assert!(out.contains("周杰伦"), "top artist 带名:{out}");
        assert!(out.contains("(未知)"), "空来源桶占位:{out}");
        assert!(out.contains("searches"), "事件量:{out}");
    }

    /// 整屏报告文本形状(窗口 + 总量 + 带名 top + 条形来源分布 + 事件量)。
    #[test]
    fn snap_report_text() {
        mineral_test::assert_snap!(
            "stats report text:窗口 + 总量 + 带名 top 三榜 + 条形来源分布 + 事件量",
            render_report(&sample_report(), "2026", /*color*/ false)
        );
    }

    /// top 榜文本形状(命中名 + 缺名回落 id)。
    #[test]
    fn snap_top_text() {
        let entries = vec![
            named("netease:1", Some("稻香"), 9, 540_000),
            named("bilibili:BV2", None, 3, 180_000),
        ];
        mineral_test::assert_snap!(
            "stats top songs text:命中名 + 缺名回落 qualified id",
            render_top(&entries, "top songs", /*color*/ false)
        );
    }
}

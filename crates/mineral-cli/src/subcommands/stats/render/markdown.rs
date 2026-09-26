//! `stats --format md` 的 markdown 渲染(年终盘点存档 / 分享形态)。

use std::path::Path;

use mineral_stats::{FinishReason, NamedEntry, PlayTail, Slice, StatsReport, StatusReport};

/// 收听 ms → `"1h30m"`。
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

/// 一张带名 top 榜的 markdown 有序列表分节;空榜省略。
fn top_block(title: &str, entries: &[NamedEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let lines = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            format!(
                "{}. {} — {} plays · {}",
                i + 1,
                display_name(e),
                e.plays,
                fmt_listen(e.listen_ms)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\n## {title}\n{lines}\n")
}

/// 「值 → 次数」分布的 markdown 表(空值占位)。
fn slice_table(slices: &[Slice]) -> String {
    let rows = slices
        .iter()
        .map(|s| {
            let value = if s.value.is_empty() {
                "(unknown)"
            } else {
                &s.value
            };
            format!("| {value} | {} |", s.plays)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("| value | plays |\n|---|---|\n{rows}")
}

/// 渲染一份盘点报告为 markdown(总览 + top 榜 + 来源分布)。
///
/// # Params:
///   - `report`: 已落名的完整报告
///   - `window`: 窗口标签("2026" / "all" 等)
///
/// # Return:
///   markdown 文档
pub fn report_md(report: &StatsReport, window: &str) -> String {
    let t = &report.totals;
    let e = &report.endurance;
    let overview = format!(
        "# Mineral Recap · {window}\n\n\
         ## Overview\n\
         - {} plays ({} completed · {} skipped)\n\
         - {} listening\n\
         - {} songs · {} active days · {} new\n\
         - {} sessions · longest {} · {} day streak",
        t.plays,
        t.completed,
        t.skipped,
        fmt_listen(t.listen_ms),
        t.distinct_songs,
        t.active_days,
        report.discoveries.new_songs.len(),
        e.sessions,
        fmt_listen(e.longest_ms),
        e.streak_days,
    );
    format!(
        "{overview}{songs}{albums}{artists}\n\n## Source Distribution\n{source}",
        songs = top_block("Top Songs", &report.top_songs),
        albums = top_block("Top Albums", &report.top_albums),
        artists = top_block("Top Artists", &report.top_artists),
        source = slice_table(&report.distributions.by_source),
    )
}

/// 渲染一张 top 榜为 markdown 有序列表。
pub fn top_md(entries: &[NamedEntry], title: &str) -> String {
    if entries.is_empty() {
        return format!("## {title}\n\n(none)");
    }
    top_block(title, entries).trim_start().to_owned()
}

/// 渲染最近播放流水为 markdown 表。
pub fn history_md(plays: &[PlayTail]) -> String {
    if plays.is_empty() {
        return "(no plays)".to_owned();
    }
    let rows = plays
        .iter()
        .map(|p| {
            format!(
                "| {} | {} | {} | {} |",
                p.started_at,
                p.song.qualified(),
                p.listen_ms,
                finish_str(p.finish_reason)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("| started (ms) | song | listened (ms) | finish |\n|---|---|---|---|\n{rows}")
}

/// 渲染埋点系统状态为 markdown。
pub fn status_md(path: &Path, size_bytes: u64, level: &str, report: &StatusReport) -> String {
    let coverage = match (report.first_play_at, report.last_play_at) {
        (Some(first), Some(last)) => format!("{first} → {last}"),
        _ => "(no plays)".to_owned(),
    };
    format!(
        "## Stats Status\n\n\
         | field | value |\n|---|---|\n\
         | db | {} |\n| size | {} bytes |\n| level | {} |\n\
         | coverage | {} |\n| plays | {} |\n| sessions | {} |\n| events | {} |",
        path.display(),
        size_bytes,
        level,
        coverage,
        report.plays,
        report.sessions,
        report.events,
    )
}

/// 结束原因的落库串。
fn finish_str(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Eof => "eof",
        FinishReason::Skip => "skip",
        FinishReason::Stop => "stop",
        FinishReason::Error => "error",
    }
}

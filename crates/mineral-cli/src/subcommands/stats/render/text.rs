//! `stats` 子命令的人读文本渲染(分节 + unicode 条形分布)。

use std::path::Path;

use mineral_stats::{
    Distributions, EventCount, FinishReason, NamedEntry, PlayTail, Slice, StatsReport, StatusReport,
};

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

/// 一条 unicode 条形(按 `value/max` 比例填 `█`,至少满格 `width`)。空 max 出空条。
fn bar(value: i64, max: i64, width: usize) -> String {
    if max <= 0 {
        return String::new();
    }
    let filled = usize::try_from(value.saturating_mul(i64::try_from(width).unwrap_or(0)) / max)
        .unwrap_or(0)
        .min(width);
    "█".repeat(filled)
}

/// 渲染埋点系统自身状态。
///
/// # Params:
///   - `path`: stats.db 路径
///   - `size_bytes`: db 文件字节数
///   - `level`: 当前采集档位串(off / core / full)
///   - `report`: 状态聚合
///
/// # Return:
///   多行状态文本
pub fn render_status(path: &Path, size_bytes: u64, level: &str, report: &StatusReport) -> String {
    let coverage = match (report.first_play_at, report.last_play_at) {
        (Some(first), Some(last)) => format!("{first} → {last}(epoch ms)"),
        _ => "(无播放记录)".to_owned(),
    };
    format!(
        "stats.db:  {}\n\
         size:      {} bytes\n\
         level:     {}\n\
         coverage:  {}\n\
         plays: {}   sessions: {}   events: {}",
        path.display(),
        size_bytes,
        level,
        coverage,
        report.plays,
        report.sessions,
        report.events,
    )
}

/// 结束原因的落库串(与 stats.db 一致)。
fn finish_str(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Eof => "eof",
        FinishReason::Skip => "skip",
        FinishReason::Stop => "stop",
        FinishReason::Error => "error",
    }
}

/// 渲染最近播放流水 tail(每行:起播 ms · qualified id · 收听 ms · 结束原因)。
pub fn render_history(plays: &[PlayTail]) -> String {
    if plays.is_empty() {
        return "(无播放记录)".to_owned();
    }
    plays
        .iter()
        .map(|p| {
            format!(
                "{}  {}  {}ms  {}",
                p.started_at,
                p.song.qualified(),
                p.listen_ms,
                finish_str(p.finish_reason)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 渲染一张 top 榜(每行:排名 · 展示名 · 次数 · 收听);`header` 是榜标题行。
pub fn render_top(entries: &[NamedEntry], header: &str) -> String {
    if entries.is_empty() {
        return format!("{header}\n  (无)");
    }
    let lines = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            format!(
                "{:>2}. {}  {} 次  {}",
                i + 1,
                display_name(e),
                e.plays,
                fmt_listen(e.listen_ms)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{header}\n{lines}")
}

/// 渲染各事件表行数(每行:行数 + 表名,0 行也列出)。
fn render_events(counts: &[EventCount]) -> String {
    if counts.is_empty() {
        return "  (无)".to_owned();
    }
    counts
        .iter()
        .map(|e| format!("  {:>8}  {}", e.count, e.table))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 渲染一组「值 → 次数」分布项(带比例条;空值显示占位)。
fn render_slices(slices: &[Slice]) -> String {
    if slices.is_empty() {
        return "  (无)".to_owned();
    }
    let max = slices.iter().map(|s| s.plays).max().unwrap_or(0);
    slices
        .iter()
        .map(|s| {
            let value = if s.value.is_empty() {
                "(未知)"
            } else {
                &s.value
            };
            format!("  {:>6}  {:<12} {}", s.plays, value, bar(s.plays, max, 20))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 一张带名 top 榜的分节(标题 + 榜体);空榜省略整节。
fn top_section(title: &str, entries: &[NamedEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    format!("\n\n{}", render_top(entries, title))
}

/// 渲染一屏盘点报告:窗口 + 总量 + 续航 / 发现 + top 歌 / 专辑 / 艺人 + 来源分布 + 事件量。
///
/// # Params:
///   - `report`: 已落名的完整报告
///   - `window`: 窗口标签("2026" / "all" 等)
///
/// # Return:
///   多行报告文本
pub fn render_report(report: &StatsReport, window: &str) -> String {
    let t = &report.totals;
    let e = &report.endurance;
    let head = format!(
        "window:      {window}\n\
         plays:       {}   completed: {}   skipped: {}\n\
         listen:      {}\n\
         songs:       {}   active days: {}   discoveries: {}\n\
         sessions:    {}   longest: {}   streak: {}d",
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
        "{head}{songs}{albums}{artists}\n\n\
         by source:\n{source}\n\n\
         events:\n{events}",
        songs = top_section("top songs:", &report.top_songs),
        albums = top_section("top albums:", &report.top_albums),
        artists = top_section("top artists:", &report.top_artists),
        source = render_source(&report.distributions),
        events = render_events(&report.events.table_counts),
    )
}

/// 来源分布分节(空则占位)。
fn render_source(dist: &Distributions) -> String {
    render_slices(&dist.by_source)
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
        let out = render_status(std::path::Path::new("/x/stats.db"), 4096, "full", &report);
        assert!(out.contains("plays: 3"), "{out}");
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
        let out = render_history(&plays);
        assert!(out.contains("netease:1"), "{out}");
        assert!(out.contains("eof"), "{out}");
        assert!(out.contains("bilibili:BV2"), "{out}");
        assert!(out.contains("skip"), "{out}");
    }

    #[test]
    fn history_empty_message() {
        assert_eq!(render_history(&[]), "(无播放记录)");
    }

    /// top 榜用回查名,缺名回落 qualified id。
    #[test]
    fn top_uses_name_or_falls_back_to_id() {
        let entries = vec![
            named("netease:1", Some("稻香"), 9, 540_000),
            named("bilibili:BV2", None, 3, 180_000),
        ];
        let out = render_top(&entries, "top songs:");
        assert!(out.contains(" 1. 稻香"), "命中名:{out}");
        assert!(out.contains(" 2. bilibili:BV2"), "缺名回落 id:{out}");
        assert!(out.contains("9 次"), "{out}");
    }

    #[test]
    fn top_empty_message() {
        assert!(render_top(&[], "top songs:").contains("(无)"));
    }

    #[test]
    fn report_shows_window_totals_and_named_tops() {
        let report = sample_report();
        let out = render_report(&report, "2026");
        assert!(out.contains("window:      2026"), "{out}");
        assert!(out.contains("plays:       12"), "{out}");
        assert!(out.contains("1h30m"), "{out}");
        assert!(out.contains("discoveries: 2"), "{out}");
        assert!(out.contains("streak: 3d"), "{out}");
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
            render_report(&sample_report(), "2026")
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
            render_top(&entries, "top songs:")
        );
    }
}

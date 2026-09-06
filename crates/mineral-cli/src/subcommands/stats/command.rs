//! `stats` 子命令的解析与执行:离线直读 stats.db(数值)+ mineral.db(名字回查),
//! 不经 daemon,WAL 下与 daemon 并发写安全。

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use color_eyre::eyre::WrapErr as _;
use mineral_stats::{ReportOptions, StatsStore};

use super::assemble::{self, TopCategory};
use super::render;
use super::window::{self, By, Format, Window, WindowDefault};

/// analytics data query (read offline)
#[derive(Debug, Subcommand)]
pub enum StatsCommand {
    /// show analytics system status
    Status {
        /// output format
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },

    /// show recent playback history
    History {
        /// time window (default: all)
        #[command(flatten)]
        window: Window,

        /// number of entries to show
        #[arg(long, default_value_t = 20)]
        limit: u32,

        /// filter by source (e.g. `netease` / `bilibili`)
        #[arg(long)]
        source: Option<String>,

        /// output format
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },

    /// query a single chart
    Top {
        /// chart category
        #[arg(value_enum)]
        category: TopCategory,

        /// time window (default: all)
        #[command(flatten)]
        window: Window,

        /// sort key
        #[arg(long, value_enum, default_value = "plays")]
        by: By,

        /// chart length
        #[arg(long)]
        limit: Option<u32>,

        /// output format
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },

    /// generate the recap report
    Report {
        /// time window (default: current year)
        #[command(flatten)]
        window: Window,

        /// override top chart length
        #[arg(long)]
        top: Option<u32>,

        /// output format
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },

    /// prune old plays
    Prune {
        /// delete plays before this day
        #[arg(long)]
        before: String,

        /// confirm the operation
        #[arg(long)]
        yes: bool,
    },

    /// clear stats.db
    Reset {
        /// confirm the operation
        #[arg(long)]
        yes: bool,
    },
}

/// 按 [`StatsCommand`] 分发到具体实现。
///
/// # Params:
///   - `command`: 已解析的 stats 子命令
///
/// # Return:
///   命令执行结果
pub async fn run(command: StatsCommand) -> color_eyre::Result<()> {
    match command {
        StatsCommand::Status { format } => status(format).await,
        StatsCommand::History {
            window,
            limit,
            source,
            format,
        } => history(&window, limit, source.as_deref(), format).await,
        StatsCommand::Top {
            category,
            window,
            by,
            limit,
            format,
        } => top(category, &window, by, limit, format).await,
        StatsCommand::Report {
            window,
            top,
            format,
        } => report(&window, top, format).await,
        StatsCommand::Prune { before, yes } => prune(&before, yes).await,
        StatsCommand::Reset { yes } => reset(yes),
    }
}

/// stats.db 路径(随 XDG data)。
fn stats_db_path() -> color_eyre::Result<PathBuf> {
    Ok(mineral_paths::data_dir()?.join("stats.db"))
}

/// 当前 Unix epoch 毫秒(窗口 / prune 截止用)。
fn now_ms() -> color_eyre::Result<i64> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .wrap_err("system time before UNIX epoch")?
        .as_millis();
    i64::try_from(ms).wrap_err("timestamp overflow")
}

/// 离线读配置的 `stats.report` 段,折算成查询期口径 [`ReportOptions`]。
///
/// 与 daemon 同一真相源:`min_listen_secs`(×1000 换算成 ms)与 `top_limit` 取自配置;
/// CLI `--top` / `--limit` 显式给出时覆盖榜长度。坏配置已在 loader 降级默认。
///
/// # Params:
///   - `top_override`: CLI 显式榜长度(`None` 则用配置 `top_limit`)
///
/// # Return:
///   装配好的查询期口径
fn report_options(top_override: Option<u32>) -> color_eyre::Result<ReportOptions> {
    let (config, _warnings) =
        mineral_config::load(&mineral_paths::config_dir()?.join("config.lua"))?;
    let report = config.stats().report();
    let min_listen_ms = i64::try_from(*report.min_listen_secs())
        .wrap_err("stats.report.min_listen_secs overflow")?
        .saturating_mul(1000);
    let top_limit = match top_override {
        Some(n) => i64::from(n),
        None => i64::try_from(*report.top_limit()).wrap_err("stats.report.top_limit overflow")?,
    };
    Ok(ReportOptions::builder()
        .min_listen_ms(min_listen_ms)
        .top_limit(top_limit)
        .build())
}

/// `stats report`:装配完整报告与展示名,按 `--format` 输出 text、json 或 markdown。
async fn report(window: &Window, top: Option<u32>, format: Format) -> color_eyre::Result<()> {
    let db_path = stats_db_path()?;
    if !db_path.exists() {
        println!("{}", render::render_absent());
        return Ok(());
    }
    let store = StatsStore::open(&db_path).await?;
    let now = now_ms()?;
    let range = window.range(WindowDefault::CurrentYear, now)?;
    let label = window.label(WindowDefault::CurrentYear, now)?;
    let opts = report_options(top)?;
    let sr = assemble::stats_report(&store, range, &opts).await?;
    let color = std::io::stdout().is_terminal();
    let out = match format {
        Format::Text => render::render_report(&sr, &label, color),
        Format::Json => {
            serde_json::to_string_pretty(&sr).wrap_err("failed to serialize report to json")?
        }
        Format::Md => render::report_md(&sr, &label),
    };
    println!("{out}");
    Ok(())
}

/// `stats top <category>`:轻量单榜,缺省全量窗;回查名后 text / json / md。
async fn top(
    category: TopCategory,
    window: &Window,
    by: By,
    limit: Option<u32>,
    format: Format,
) -> color_eyre::Result<()> {
    let db_path = stats_db_path()?;
    if !db_path.exists() {
        println!("{}", render::render_absent());
        return Ok(());
    }
    let store = StatsStore::open(&db_path).await?;
    let range = window.range(WindowDefault::All, now_ms()?)?;
    let opts = report_options(limit)?;
    let entries = assemble::top_entries(&store, category, range, by.into(), &opts).await?;
    let color = std::io::stdout().is_terminal();
    let out = match format {
        Format::Text => render::render_top(&entries, category.text_title(), color),
        Format::Json => {
            serde_json::to_string_pretty(&entries).wrap_err("failed to serialize top to json")?
        }
        Format::Md => render::top_md(&entries, category.md_title()),
    };
    println!("{out}");
    Ok(())
}

/// `stats history`:最近播放流水 tail,缺省全量窗、可按来源过滤。
async fn history(
    window: &Window,
    limit: u32,
    source: Option<&str>,
    format: Format,
) -> color_eyre::Result<()> {
    let db_path = stats_db_path()?;
    if !db_path.exists() {
        println!("{}", render::render_absent());
        return Ok(());
    }
    let store = StatsStore::open(&db_path).await?;
    let range = window.range(WindowDefault::All, now_ms()?)?;
    let plays = store.recent_plays(range, source, i64::from(limit)).await?;
    let color = std::io::stdout().is_terminal();
    let out = match format {
        Format::Text => render::render_history(&plays, color),
        Format::Json => {
            serde_json::to_string_pretty(&plays).wrap_err("failed to serialize history to json")?
        }
        Format::Md => render::history_md(&plays),
    };
    println!("{out}");
    Ok(())
}

/// `stats status`:直读 stats.db + 离线读配置取 level;不存在则友好提示,不报错栈。
async fn status(format: Format) -> color_eyre::Result<()> {
    let db_path = stats_db_path()?;
    if !db_path.exists() {
        println!("{}", render::render_absent());
        return Ok(());
    }
    // 离线自 eval 配置取当前 level(与 daemon 同一真相源);坏配置已在 loader 降级默认。
    let (config, _warnings) =
        mineral_config::load(&mineral_paths::config_dir()?.join("config.lua"))?;
    let level = match config.stats().level() {
        mineral_config::StatsLevel::Off => "off",
        mineral_config::StatsLevel::Core => "core",
        mineral_config::StatsLevel::Full => "full",
    };
    let store = StatsStore::open(&db_path).await?;
    let report = store.status().await?;
    let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    let color = std::io::stdout().is_terminal();
    let out = match format {
        Format::Text => render::render_status(&db_path, size, level, &report, color),
        Format::Json => serde_json::to_string_pretty(&serde_json::json!({
            "path": db_path.display().to_string(),
            "size_bytes": size,
            "level": level,
            "plays": report.plays,
            "sessions": report.sessions,
            "events": report.events,
            "first_play_at": report.first_play_at,
            "last_play_at": report.last_play_at,
        }))
        .wrap_err("failed to serialize status to json")?,
        Format::Md => render::status_md(&db_path, size, level, &report),
    };
    println!("{out}");
    Ok(())
}

/// `stats prune --before <date>`:删该日零点(UTC)之前的流水;无 `--yes` 只打印将删行数。
async fn prune(before: &str, yes: bool) -> color_eyre::Result<()> {
    let db_path = stats_db_path()?;
    if !db_path.exists() {
        println!("{}", render::render_absent());
        return Ok(());
    }
    let cutoff = window::day_start_ms(before).wrap_err("invalid --before date")?;
    let store = StatsStore::open(&db_path).await?;
    if !yes {
        let n = store.count_before(cutoff).await?;
        println!(
            "Will delete {n} rows before {before} (plays + event tables + stale sessions). Run with --yes to execute."
        );
        return Ok(());
    }
    store.prune(cutoff).await?;
    println!("Deleted plays before {before}.");
    Ok(())
}

/// `stats reset`:清空 stats.db + `-wal`/`-shm` 伴生文件(沿用 `cache reset` 惯例:无
/// `--yes` 只打印计划)。
fn reset(yes: bool) -> color_eyre::Result<()> {
    let db = mineral_paths::data_dir()?.join("stats.db");
    let siblings = [
        db.clone(),
        db.with_extension("db-wal"),
        db.with_extension("db-shm"),
    ];
    let existing = siblings
        .into_iter()
        .filter(|p| p.exists())
        .collect::<Vec<_>>();
    if existing.is_empty() {
        println!("stats.db does not exist; nothing to clear.");
        return Ok(());
    }
    if !yes {
        println!("Will delete (run with --yes):");
        for p in &existing {
            println!("  {}", p.display());
        }
        return Ok(());
    }
    for p in &existing {
        remove(p)?;
    }
    println!("Cleared stats.db (deleted {} file(s))", existing.len());
    Ok(())
}

/// 删单个文件(不存在视为已删,不报错)。
fn remove(path: &Path) -> color_eyre::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).wrap_err_with(|| format!("failed to delete {}", path.display())),
    }
}

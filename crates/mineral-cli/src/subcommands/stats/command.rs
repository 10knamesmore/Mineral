//! `stats` 子命令的解析与执行:离线直读 stats.db(数值)+ mineral.db(名字回查),
//! 不经 daemon,WAL 下与 daemon 并发写安全。

use std::path::{Path, PathBuf};

use clap::Subcommand;
use color_eyre::eyre::WrapErr as _;
use mineral_persist::ServerStore;
use mineral_stats::{ReportOptions, StatsStore};

use super::assemble::{self, NameResolver, TopCategory};
use super::render;
use super::window::{self, By, Format, Window, WindowDefault};

/// 埋点数据查询(离线直读)。
#[derive(Debug, Subcommand)]
pub enum StatsCommand {
    /// 埋点系统自身状态:db 路径 / 大小 / 当前 level / 时间覆盖 / 各区行数。
    Status {
        /// 输出格式。
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },

    /// 最近播放流水 tail(「昨晚听了啥」/ 验证埋点在记)。
    History {
        /// 时间窗(缺省全量)。
        #[command(flatten)]
        window: Window,

        /// 展示条数(0 无意义,负值在解析层即拒)。
        #[arg(long, default_value_t = 20)]
        limit: u32,

        /// 只看某来源(来源 name,如 `netease` / `bilibili`)。
        #[arg(long)]
        source: Option<String>,

        /// 输出格式。
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },

    /// 轻量单榜查询(`playlists` = 队列上下文口径:最常从哪个歌单起播)。
    Top {
        /// 榜类别。
        #[arg(value_enum)]
        category: TopCategory,

        /// 时间窗(缺省全量)。
        #[command(flatten)]
        window: Window,

        /// 排序口径。
        #[arg(long, value_enum, default_value = "plays")]
        by: By,

        /// 榜单长度(缺省来自配置 `stats.report.top_limit`;负值在解析层即拒)。
        #[arg(long)]
        limit: Option<u32>,

        /// 输出格式。
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },

    /// 盘点报告主入口(§8.1 全套装配 + mineral.db 回查名)。
    Report {
        /// 时间窗(缺省当前年)。
        #[command(flatten)]
        window: Window,

        /// 覆盖各 top 榜长度(缺省来自配置 `stats.report.top_limit`;负值在解析层即拒)。
        #[arg(long)]
        top: Option<u32>,

        /// 输出格式(`md` 适合年终存档:`stats report --format md > wrapped.md`)。
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },

    /// 一次性裁剪:删 `--before` 日期之前的流水;无 `--yes` 只打印将删行数,不动盘。
    Prune {
        /// 裁剪水位 `YYYY-MM-DD`:早于当日零点(UTC)的流水被删。
        #[arg(long)]
        before: String,

        /// 确认执行;不带此 flag 只 dry-run。
        #[arg(long)]
        yes: bool,
    },

    /// 清空 stats.db(连同 -wal/-shm 伴生文件);无 `--yes` 只打印将删的文件。
    Reset {
        /// 确认执行;不带此 flag 只打印计划,不动盘。
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
        .wrap_err("系统时间早于 UNIX epoch")?
        .as_millis();
    i64::try_from(ms).wrap_err("时间戳溢出 i64")
}

/// 开 mineral.db 只读做名字回查;库不存在则用降级句柄(名字全回落 id,不建空库)。
async fn open_resolver() -> color_eyre::Result<NameResolver> {
    let mineral_db = mineral_paths::data_dir()?.join("mineral.db");
    let persist = if mineral_db.exists() {
        ServerStore::open(&mineral_db).await?
    } else {
        ServerStore::disabled()
    };
    Ok(NameResolver::new(persist))
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
        .wrap_err("stats.report.min_listen_secs 溢出 i64")?
        .saturating_mul(1000);
    let top_limit = match top_override {
        Some(n) => i64::from(n),
        None => i64::try_from(*report.top_limit()).wrap_err("stats.report.top_limit 溢出 i64")?,
    };
    Ok(ReportOptions::builder()
        .min_listen_ms(min_listen_ms)
        .top_limit(top_limit)
        .build())
}

/// `stats report`:§8.1 全套装配 + 回查名,按 `--format` 出 text / json / md。
async fn report(window: &Window, top: Option<u32>, format: Format) -> color_eyre::Result<()> {
    let db_path = stats_db_path()?;
    if !db_path.exists() {
        println!("{}", render::render_absent());
        return Ok(());
    }
    let store = StatsStore::open(&db_path).await?;
    let resolver = open_resolver().await?;
    let now = now_ms()?;
    let range = window.range(WindowDefault::CurrentYear, now)?;
    let label = window.label(WindowDefault::CurrentYear, now)?;
    let opts = report_options(top)?;
    let sr = assemble::stats_report(&store, &resolver, range, &opts).await?;
    let out = match format {
        Format::Text => render::render_report(&sr, &label),
        Format::Json => serde_json::to_string_pretty(&sr).wrap_err("report json 序列化失败")?,
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
    let resolver = open_resolver().await?;
    let range = window.range(WindowDefault::All, now_ms()?)?;
    let opts = report_options(limit)?;
    let entries =
        assemble::top_entries(&store, &resolver, category, range, by.into(), &opts).await?;
    let out = match format {
        Format::Text => render::render_top(&entries, category.text_title()),
        Format::Json => serde_json::to_string_pretty(&entries).wrap_err("top json 序列化失败")?,
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
    let out = match format {
        Format::Text => render::render_history(&plays),
        Format::Json => serde_json::to_string_pretty(&plays).wrap_err("history json 序列化失败")?,
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
    let out = match format {
        Format::Text => render::render_status(&db_path, size, level, &report),
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
        .wrap_err("status json 序列化失败")?,
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
    let cutoff = window::day_start_ms(before).wrap_err("--before 日期无效")?;
    let store = StatsStore::open(&db_path).await?;
    if !yes {
        let n = store.count_before(cutoff).await?;
        println!("将删除 {before} 之前的 {n} 行流水(plays + 事件表 + 旧会话);加 --yes 执行。");
        return Ok(());
    }
    store.prune(cutoff).await?;
    println!("已删除 {before} 之前的流水。");
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
        println!("stats.db 不存在,无需清空。");
        return Ok(());
    }
    if !yes {
        println!("将删除(加 --yes 执行):");
        for p in &existing {
            println!("  {}", p.display());
        }
        return Ok(());
    }
    for p in &existing {
        remove(p)?;
    }
    println!("已清空 stats.db（删 {} 个文件）", existing.len());
    Ok(())
}

/// 删单个文件(不存在视为已删,不报错)。
fn remove(path: &Path) -> color_eyre::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).wrap_err_with(|| format!("删除 {} 失败", path.display())),
    }
}

//! Mineral 全局日志 facade。
//!
//! 对外:re-export `tracing` 的 [`trace!`] / [`debug!`] / [`info!`] / [`warn!`] / [`error!`]
//! 与 [`instrument`]、[`span!`]、[`event!`] —— 业务代码 `use mineral_log::warn;` 即可,
//! 不需要直接依赖 `tracing`。
//!
//! 后端:[`init`] 安装一个 `tracing-subscriber`,把日志写到
//! `<cache_dir>/mineral.log.YYYY-MM-DD`(daily-rolling,non-blocking writer)。
//! 过滤档位走 `RUST_LOG`,缺省 `info`。
//!
//! 用法:
//!
//! ```ignore
//! fn main() -> color_eyre::Result<()> {
//!     // 进程入口处调一次,guard 必须持到退出
//!     let _log_guard = mineral_log::init()?;
//!     // ...
//! }
//! ```
//!
//! ```ignore
//! mineral_log::warn!(target: "channel_fetch", ?source, "no channel registered");
//! ```

pub use tracing::{Level, debug, error, event, info, instrument, span, trace, warn};

use color_eyre::eyre::{WrapErr, eyre};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::ChronoLocal;

/// 把错误渲染成**单行的完整 context 链**(eyre 的 `{:#}`),供日志 `error` 字段用。
///
/// 三种格式化的取舍:
/// - `{}`(tracing `%e`):只最外层一条 message,`.wrap_err()` 加的 context 全丢,查问题看不全。
/// - `{:?}`(tracing `?e`):color-eyre 的 Debug —— 带 ANSI 颜色 + `Location` + `Backtrace`,
///   是给终端看的「报告」,塞进日志字段就是一坨噪音(还污染纯文本日志)。
/// - `{:#}`(本函数):完整 context 链用 `: ` 串成单行,无颜色、无 backtrace —— 既全又干净。
///
/// # Params:
///   - `e`: 任意实现 `Display` 的错误(eyre `Report` / `thiserror` enum / std error 皆可)。
///
/// # Return:
///   形如 `顶层 context: 中层: 底层` 的单行字符串。
pub fn chain(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// 滚动日志文件名前缀;tracing-appender 会附加 `.YYYY-MM-DD`。
const LOG_FILE_PREFIX: &str = "mineral.log";

/// 安装全局日志 subscriber,返回的 [`WorkerGuard`] 必须持到进程退出
/// (drop 它会停掉后台 flush 线程,后续日志静默丢失)。
///
/// 行为:
/// - 文件:`<cache_dir>/mineral.log.YYYY-MM-DD`,daily 轮转,non-blocking
/// - 格式:本地时间(`HH:MM:SS.mmm`,日期见文件名)+ 级别 + target + `file:line` + 消息/字段
/// - 过滤:`RUST_LOG` 环境变量,缺省 `info`;额外压低 symphonia / isahc 等第三方噪音
/// - 不输出到 stdout/stderr(避免与 TUI alternate screen 撞)
///
/// # Return:
///   `WorkerGuard` —— 在 `main` 顶层 `let _g = ...?;` 持有即可。
pub fn init() -> color_eyre::Result<WorkerGuard> {
    let log_dir = mineral_paths::cache_dir().wrap_err("locate cache dir for log")?;
    std::fs::create_dir_all(&log_dir)
        .wrap_err_with(|| format!("create log dir {}", log_dir.display()))?;

    let appender = tracing_appender::rolling::daily(&log_dir, LOG_FILE_PREFIX);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    // 第三方噪音压制指令放在 base 之前:base(`RUST_LOG` 或缺省 `info`)里用户对同一
    // target 的显式指令优先级更高,可覆盖这里的默认压制;而缺省的全局 `info` 不会盖掉
    // 更具体的 per-target 指令,于是 symphonia / isahc 的刷屏被压住、其余照常 info。
    let base = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned());
    let filter = EnvFilter::new(format!(
        "symphonia=warn,symphonia_bundle_mp3=error,isahc=error,{base}"
    ));

    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .with_timer(ChronoLocal::new("%H:%M:%S%.3f".to_owned()))
        .try_init()
        .map_err(|e| eyre!("install tracing subscriber: {e}"))?;

    Ok(guard)
}

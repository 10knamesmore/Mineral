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

/// 滚动日志文件名前缀;tracing-appender 会附加 `.YYYY-MM-DD`。
const LOG_FILE_PREFIX: &str = "mineral.log";

/// 安装全局日志 subscriber,返回的 [`WorkerGuard`] 必须持到进程退出
/// (drop 它会停掉后台 flush 线程,后续日志静默丢失)。
///
/// 行为:
/// - 文件:`<cache_dir>/mineral.log.YYYY-MM-DD`,daily 轮转,non-blocking
/// - 过滤:`RUST_LOG` 环境变量,缺省 `info`
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

    let filter = match EnvFilter::try_from_default_env() {
        Ok(f) => f,
        Err(_) => EnvFilter::new("info"),
    };

    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .try_init()
        .map_err(|e| eyre!("install tracing subscriber: {e}"))?;

    Ok(guard)
}

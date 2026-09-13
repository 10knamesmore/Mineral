//! Mineral 全局日志 facade。
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

#[doc(hidden)]
pub use tracing as __tracing;

mod macros;

use color_eyre::eyre::{WrapErr, eyre};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::ChronoLocal;

/// 把错误渲染成**单行的完整 context 链**(eyre 的 `{:#}`),供日志 `error` 字段用。
pub fn chain(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// 滚动日志文件名前缀;tracing-appender 会附加 `.YYYY-MM-DD`。
const LOG_FILE_PREFIX: &str = "mineral.log";

/// 日志文件总数上限(含当前文件);启动与每日轮转时由 appender 先删除最早创建的文件。
const MAX_LOG_FILES: usize = 7;

/// 日志文件所在目录(`<cache_dir>`),给上层做「详见日志」类提示用。
///
/// # Return:
///   日志目录路径;定位 cache dir 失败时返回 `Err`。
pub fn log_dir() -> color_eyre::Result<std::path::PathBuf> {
    mineral_paths::cache_dir().wrap_err("locate cache dir for log")
}

/// 安装全局日志 subscriber,返回的 [`WorkerGuard`] 必须持到进程退出
/// (drop 它会停掉后台 flush 线程,后续日志静默丢失)。
///
/// 行为:
/// - 文件:`<cache_dir>/mineral.log.YYYY-MM-DD`,daily 轮转,non-blocking
/// - 格式:本地时间(`HH:MM:SS.mmm`,日期见文件名)+ 级别 + target + `file:line` + 消息/字段
/// - 不输出到 stdout/stderr(避免与 TUI alternate screen 撞)
///
/// # Return:
///   `WorkerGuard` —— 在 `main` 顶层 `let _g = ...?;` 持有即可。
pub fn init() -> color_eyre::Result<WorkerGuard> {
    let appender = file_appender(&log_dir()?)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let directives = std::env::var("RUST_LOG").ok();
    let filter = log_filter(directives.as_deref());

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

/// 创建日志目录并打开当日日志;appender 在启动及轮转时清理超额的旧日志。
fn file_appender(directory: &std::path::Path) -> color_eyre::Result<RollingFileAppender> {
    std::fs::create_dir_all(directory)
        .wrap_err_with(|| format!("create log dir {}", directory.display()))?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(directory)
        .wrap_err_with(|| format!("open rolling log in {}", directory.display()))
}

/// 合成默认级别与 `RUST_LOG`;用户同名 target 指令覆盖默认值,更具体的 target 优先。
fn log_filter(directives: Option<&str>) -> EnvFilter {
    let mut filter = "warn,mineral=info".to_owned();
    if let Some(directives) = directives {
        filter.push(',');
        filter.push_str(directives);
    }
    EnvFilter::new(filter)
}

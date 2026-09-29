//! Mineral 全局日志 facade。
//!
//! 用法:
//!
//! ```ignore
//! fn main() -> mineral_log::Result<()> {
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

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::ChronoLocal;

/// 日志初始化与文件操作失败。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 无法解析日志目录。
    #[error("locate cache dir for log")]
    LogDirectory(#[source] mineral_paths::Error),

    /// 无法创建日志目录。
    #[error("create log dir {}", .path.display())]
    CreateDirectory {
        /// 日志目录。
        path: std::path::PathBuf,

        /// 文件系统错误。
        #[source]
        source: std::io::Error,
    },

    /// 无法打开滚动日志文件。
    #[error("open rolling log in {}", .path.display())]
    OpenAppender {
        /// 日志目录。
        path: std::path::PathBuf,

        /// appender 构建错误。
        #[source]
        source: tracing_appender::rolling::InitError,
    },

    /// 无法安装进程级 tracing subscriber。
    #[error("install tracing subscriber")]
    Subscriber(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// 日志初始化与路径解析操作的结果。
pub type Result<T> = std::result::Result<T, Error>;

/// 把错误及其 source 链渲染成单行,供日志 `error` 字段使用。
pub fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        text.push_str(": ");
        text.push_str(&next.to_string());
        source = next.source();
    }
    text.replace(['\n', '\r'], " ")
}

/// 滚动日志文件名前缀;tracing-appender 会附加 `.YYYY-MM-DD`。
const LOG_FILE_PREFIX: &str = "mineral.log";

/// 日志文件总数上限(含当前文件);启动与每日轮转时由 appender 先删除最早创建的文件。
const MAX_LOG_FILES: usize = 7;

/// 日志文件所在目录(`<cache_dir>`),给上层做「详见日志」类提示用。
///
/// # Return:
///   日志目录路径;定位 cache dir 失败时返回 `Err`。
pub fn log_dir() -> Result<std::path::PathBuf> {
    mineral_paths::cache_dir().map_err(Error::LogDirectory)
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
pub fn init() -> Result<WorkerGuard> {
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
        .map_err(Error::Subscriber)?;

    Ok(guard)
}

/// 创建日志目录并打开当日日志;appender 在启动及轮转时清理超额的旧日志。
fn file_appender(directory: &std::path::Path) -> Result<RollingFileAppender> {
    std::fs::create_dir_all(directory).map_err(|source| Error::CreateDirectory {
        path: directory.to_path_buf(),
        source,
    })?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(directory)
        .map_err(|source| Error::OpenAppender {
            path: directory.to_path_buf(),
            source,
        })
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

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::{Error, chain};

    /// 日志链包括嵌套 source,同时折叠 source 中的换行。
    #[test]
    fn renders_all_sources_on_one_line() -> std::io::Result<()> {
        let source =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission\ndenied");
        let error = Error::LogDirectory(mineral_paths::Error::RuntimeDirectory {
            operation: "创建",
            path: "/tmp/mineral".into(),
            source,
        });
        let rendered = chain(&error);
        let leaf = error
            .source()
            .and_then(|cause| cause.source())
            .ok_or_else(|| std::io::Error::other("missing runtime directory IO source"))?;
        assert!(rendered.contains(&leaf.to_string().replace('\n', " ")));
        assert!(!rendered.contains('\n'));
        Ok(())
    }
}

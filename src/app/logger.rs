use anyhow::{Context, Result};
use once_cell::sync::OnceCell;
use std::{fs, path::Path};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::time::OffsetTime;

static LOGGER_GUARD: OnceCell<WorkerGuard> = OnceCell::new();

pub fn init<P: AsRef<Path>>(log_path: P) -> Result<()> {
    let time_format = time::format_description::parse(
        "[year]-[month padding:zero]-[day padding:zero] [hour]:[minute]:[second]",
    )
    .context("invalid format string")?;
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let timer = OffsetTime::new(offset, time_format);

    if let Some(parent) = log_path.as_ref().parent() {
        fs::create_dir_all(parent).with_context(|| format!("创建日志目录 {:?} 时失败", parent))?;
    }

    let file = std::fs::File::create(log_path.as_ref())
        .with_context(|| format!("无法创建日志文件: {:?}", log_path.as_ref()))?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    LOGGER_GUARD
        .set(guard)
        .map_err(|_| anyhow::anyhow!("Logger 重复初始化"))?;

    let subscriber = tracing_subscriber::fmt()
        .with_timer(timer)
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_names(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .context("设置全局 tracing subscriber 失败")?;

    Ok(())
}

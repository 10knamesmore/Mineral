use anyhow::Context;
use std::fs;
use tracing_appender::{non_blocking::WorkerGuard, rolling};
use tracing_subscriber::EnvFilter;

const APP_NAME: &str = "mineral";

pub fn init() -> anyhow::Result<WorkerGuard> {
    let log_dir = mineral_platform::dir::logs_dir();

    fs::create_dir_all(&log_dir).with_context(|| format!("创建日志目录 {:?} 时失败", log_dir))?;

    let file_appender = rolling::daily(&log_dir, format!("{}.log", APP_NAME));

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_names(true)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init()
        .map_err(|e| anyhow::anyhow!("初始化日志系统失败: {}", e))?;

    Ok(guard)
}

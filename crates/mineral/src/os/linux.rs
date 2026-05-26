//! 非 macOS 平台的 daemon 入口:主线程直接 block_on tokio runtime。

/// daemon 入口(`mineral serve`):主线程直接跑 tokio runtime。
pub(crate) fn run_daemon() -> color_eyre::Result<()> {
    crate::serve_blocking()
}

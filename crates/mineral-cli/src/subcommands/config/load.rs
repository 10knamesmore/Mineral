//! 非配置 CLI 命令在阻塞任务中读取 daemon 设置;不执行 setup。

use mineral_config::ConfigWarning;
use mineral_server::config::DaemonConfig;

/// 获取离线 daemon 设置与诊断,文件求值不占用异步 worker。
pub(crate) async fn daemon() -> crate::error::Result<(DaemonConfig, Vec<ConfigWarning>)> {
    let path = mineral_paths::config_dir()?.join("daemon.lua");
    Ok(
        tokio::task::spawn_blocking(move || mineral_server::config::load_daemon(&path))
            .await
            .map_err(crate::error::Error::ConfigTask)??,
    )
}

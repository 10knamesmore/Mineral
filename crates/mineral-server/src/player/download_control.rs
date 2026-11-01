//! 会话下载的提交、取消、状态查询与关停等待。

use mineral_protocol::{DownloadId, DownloadTarget};
use tokio::sync::{oneshot, watch};

use super::PlayerCore;
use crate::download::{DownloadSnapshot, StopError};

impl PlayerCore {
    /// 下载管理任务合并发布的只读快照，供领域发布器使用。
    pub(crate) fn download_snapshots(&self) -> watch::Receiver<DownloadSnapshot> {
        self.inner.downloads.snapshots()
    }

    /// Submits a Song or playlist to the session download manager.
    pub(crate) fn download(&self, target: DownloadTarget) {
        self.inner.downloads.submit(target)
    }

    /// Waits for Stop acceptance, not for an active writer to exit.
    pub(crate) async fn stop_download(&self, id: &DownloadId) -> Result<(), StopError> {
        self.inner.downloads.stop(id).await
    }

    /// Cancels and waits for all active Song downloads during daemon shutdown.
    pub(crate) async fn shutdown_downloads(&self) -> Result<(), oneshot::error::RecvError> {
        self.inner.downloads.shutdown().await
    }
}

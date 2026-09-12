//! 会话下载的提交、取消、状态查询与关停等待。

use mineral_protocol::{DownloadId, DownloadSummary, DownloadTarget, SongDownloadView};

use super::PlayerCore;

impl PlayerCore {
    /// Returns the current small download summary.
    pub(crate) fn download_summary(&self) -> DownloadSummary {
        self.inner.downloads.summary()
    }

    /// Returns the current flat Song download snapshot.
    pub(crate) fn download_snapshot(&self) -> Vec<SongDownloadView> {
        self.inner.downloads.snapshot()
    }

    /// 下载明细变更订阅端(领域发布器用)。
    pub(crate) fn download_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.downloads.changes()
    }

    /// Submits a Song or playlist to the session download manager.
    pub(crate) fn download(&self, target: DownloadTarget) {
        self.inner.downloads.submit(target)
    }

    /// Stops one queued or active Song download.
    pub(crate) fn stop_download(&self, id: &DownloadId) -> color_eyre::Result<()> {
        self.inner.downloads.stop(id)
    }

    /// Cancels and waits for all active Song downloads during daemon shutdown.
    pub(crate) async fn shutdown_downloads(&self) {
        self.inner.downloads.shutdown().await;
    }
}

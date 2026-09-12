//! 下载任务的停止请求与执行结果。

use mineral_protocol::DownloadId;

use super::Client;
use crate::operation::{Pending, SubmitError, decode_applied};

impl Client {
    /// Stop 一个下载(不等待结论,结果句柄交给调用方 / 完成事件队列)。
    ///
    /// # Params:
    ///   - `id`: 下载 id
    ///
    /// # Errors
    /// 本地未提交。
    pub fn stop_download_pending(&self, id: DownloadId) -> Result<Pending<()>, SubmitError> {
        self.submit(mineral_protocol::Request::StopDownload(id), decode_applied)
    }
}

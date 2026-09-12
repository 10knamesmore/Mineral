//! 请求执行结果的交付与失败诊断。

use mineral_protocol::{OperationResult, RequestId};
use tokio::sync::oneshot;

/// 请求结果的处理去向，由提交方选择并随在途请求保留。
pub(crate) enum ResultTarget {
    /// 将结论交给等待者；断连时丢弃发送端，让等待者得知结果未知。
    Caller(oneshot::Sender<OperationResult>),

    /// 调用者不等待结论，由会话在收到业务失败时记日志。
    LogFailures {
        /// 请求名称，用于定位失败操作。
        request_name: &'static str,
    },
}

impl ResultTarget {
    /// 处理一条已收到的 daemon 结论。
    ///
    /// # Params:
    ///   - `id`: 结论对应的会话内请求标识。
    ///   - `result`: daemon 返回的执行结论。
    pub(super) fn deliver(self, id: RequestId, result: OperationResult) {
        match self {
            Self::Caller(sender) => {
                drop(sender.send(result));
            }
            Self::LogFailures { request_name } => {
                if let OperationResult::Failed(failure) = result {
                    mineral_log::warn!(
                        target: "ipc",
                        method = request_name,
                        request_id = id.value(),
                        kind = ?failure.kind,
                        detail = failure.detail,
                        "daemon 拒绝操作"
                    );
                }
            }
        }
    }
}

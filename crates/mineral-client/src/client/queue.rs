//! 队列替换、起播与结构编辑。

use mineral_model::Song;
use mineral_protocol::{
    FailureKind, OperationResult, PlayQueueError, QueueContextWire, QueueEditOutcome, QueueOp,
};

use super::Client;
use crate::operation::{Outcome, Pending, SubmitError};

impl Client {
    /// 原子替换队列并起播目标位置。
    ///
    /// 成功返回 `Applied(())`;业务失败(空队列 / 超容量 / 目标越界)返回 `Failed`。
    ///
    /// # Params:
    ///   - `songs`: 新队列
    ///   - `target`: 队列内 0-based 起播下标
    ///   - `context`: 队列来源语境
    ///
    /// # Errors
    /// 本地未提交。
    pub fn play_queue(
        &self,
        songs: Vec<Song>,
        target: usize,
        context: QueueContextWire,
    ) -> Result<Pending<()>, SubmitError> {
        self.submit(
            mineral_protocol::Request::PlayQueue {
                songs,
                target,
                context,
            },
            |result, request_name| match result {
                OperationResult::Query(response) => match *response {
                    mineral_protocol::Response::PlayQueue(Ok(())) => Outcome::Applied(()),
                    mineral_protocol::Response::PlayQueue(Err(error)) => Outcome::Failed {
                        kind: {
                            match error {
                                PlayQueueError::Empty
                                | PlayQueueError::CapacityExceeded { .. }
                                | PlayQueueError::TargetOutOfBounds { .. } => FailureKind::Invalid,
                                PlayQueueError::Unavailable { .. } => FailureKind::Unavailable,
                            }
                        },
                        detail: error.to_string(),
                    },
                    other => Outcome::Failed {
                        kind: FailureKind::Internal,
                        detail: format!("{request_name} 收到意外应答 {other:?}"),
                    },
                },
                OperationResult::Failed(failure) => Outcome::Failed {
                    kind: failure.kind,
                    detail: failure.detail,
                },
                _ => Outcome::Failed {
                    kind: FailureKind::Internal,
                    detail: format!("{request_name} 缺少结果载荷"),
                },
            },
        )
    }

    /// 队列结构编辑(等待结论)。
    ///
    /// # Params:
    ///   - `op`: 编辑操作
    pub async fn queue_edit(&self, op: QueueOp) -> Outcome<QueueEditOutcome> {
        match self.queue_edit_pending(op) {
            Ok(pending) => pending.outcome().await,
            Err(error) => Outcome::Unknown {
                detail: format!("未提交:{error}"),
            },
        }
    }

    /// 队列结构编辑(不等待结论,结果句柄交给调用方 / 完成事件队列)。
    ///
    /// # Params:
    ///   - `op`: 编辑操作
    ///
    /// # Errors
    /// 本地未提交。
    pub fn queue_edit_pending(
        &self,
        op: QueueOp,
    ) -> Result<Pending<QueueEditOutcome>, SubmitError> {
        self.submit(
            mineral_protocol::Request::QueueEdit { op },
            |result, request_name| match result {
                OperationResult::Query(response) => match *response {
                    mineral_protocol::Response::QueueEdited(outcome) => Outcome::Applied(outcome),
                    other => Outcome::Failed {
                        kind: FailureKind::Internal,
                        detail: format!("{request_name} 收到意外应答 {other:?}"),
                    },
                },
                OperationResult::Applied => Outcome::Applied(QueueEditOutcome::Applied),
                OperationResult::Accepted => Outcome::Accepted(QueueEditOutcome::Applied),
                OperationResult::Failed(failure) => Outcome::Failed {
                    kind: failure.kind,
                    detail: failure.detail,
                },
            },
        )
    }
}

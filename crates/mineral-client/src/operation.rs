//! 区分请求是否在本地提交，以及 daemon 是否已执行或受理操作。
//!
//! 返回 `Result<Pending<T>, SubmitError>` 的方法在本地拒绝请求时立即返回错误；
//! 提交成功后，调用 [`Pending::outcome`] 获取执行结论。异步便捷方法直接返回
//! [`Outcome`]，也可能把本地提交失败表示为 `Unknown`。
//! `Applied` 表示操作已应用，`Accepted` 表示后台工作已受理，完成状态由后续推送表达。
//! `Failed` 携带业务失败类别，`Unknown` 表示无法确认结论；断连后不自动重发。

use mineral_protocol::{FailureKind, OperationResult};
use tokio::sync::oneshot;

/// 操作结论。
#[derive(Clone, Debug)]
pub enum Outcome<T> {
    /// 操作已在 daemon 应用;client 镜像在收到订阅更新后反映该状态。
    Applied(T),

    /// 后台工作已受理(完成由领域状态表达)。
    Accepted(T),

    /// 业务失败(daemon 给出的结构化结论)。
    Failed {
        /// 失败类别。
        kind: FailureKind,

        /// 诊断细节。
        detail: String,
    },

    /// 无法确认执行结论：可能本地未提交，也可能等待期间断连；不自动重发。
    Unknown {
        /// 人读细节。
        detail: String,
    },
}

impl<T> Outcome<T> {
    /// 是否成功(已应用 / 已受理)。
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Applied(_) | Self::Accepted(_))
    }

    /// 成功时取出载荷。
    #[must_use]
    pub fn into_success(self) -> Option<T> {
        match self {
            Self::Applied(value) | Self::Accepted(value) => Some(value),
            Self::Failed { .. } | Self::Unknown { .. } => None,
        }
    }

    /// 把载荷映射成另一种类型。
    ///
    /// # Params:
    ///   - `f`: 映射函数
    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Self::Applied(value) => Outcome::Applied(f(value)),
            Self::Accepted(value) => Outcome::Accepted(f(value)),
            Self::Failed { kind, detail } => Outcome::Failed { kind, detail },
            Self::Unknown { detail } => Outcome::Unknown { detail },
        }
    }
}

/// 一条已提交请求的结果句柄。
pub struct Pending<T> {
    /// daemon 结论接收端。
    rx: oneshot::Receiver<OperationResult>,

    /// 协议结论 → 业务结论的译码。
    decode: Box<dyn FnOnce(OperationResult) -> Outcome<T> + Send>,
}

impl<T> Pending<T> {
    /// 将已登记请求的应答接收端与结论译码配对。
    ///
    /// # Params:
    ///   - `rx`: 会话投递 daemon 应答的接收端。
    ///   - `decode`: 将应答载荷转换为操作结论。
    ///
    /// # Return:
    ///   供调用方等待执行结论的句柄。
    pub(crate) fn new(
        rx: oneshot::Receiver<OperationResult>,
        decode: impl FnOnce(OperationResult) -> Outcome<T> + Send + 'static,
    ) -> Self {
        Self {
            rx,
            decode: Box::new(decode),
        }
    }

    /// 造一个立即就绪的结果(测试替身 / 本地短路用)。
    ///
    /// # Params:
    ///   - `outcome`: 预设结论
    #[must_use]
    pub fn ready(outcome: Outcome<T>) -> Self
    where
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        drop(tx.send(OperationResult::Applied));
        Self {
            rx,
            decode: Box::new(move |_| outcome),
        }
    }

    /// 等待结论。
    pub async fn outcome(self) -> Outcome<T> {
        let Self { rx, decode } = self;
        match rx.await {
            Ok(result) => decode(result),
            Err(_closed) => Outcome::Unknown {
                detail: "会话在结果到达前结束".to_owned(),
            },
        }
    }
}

/// 本地未提交的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    /// 待发队列已满。
    QueueFull,

    /// 在途请求达到上限。
    InFlightLimit,

    /// 会话已断开 / 已关闭。
    Disconnected,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => f.write_str("本地待发队列已满"),
            Self::InFlightLimit => f.write_str("在途请求已达上限"),
            Self::Disconnected => f.write_str("会话已断开"),
        }
    }
}

impl std::error::Error for SubmitError {}

/// 译码「已应用 / 已受理」类结论。
pub(crate) fn decode_applied(result: OperationResult, request_name: &'static str) -> Outcome<()> {
    match result {
        OperationResult::Applied | OperationResult::Accepted => Outcome::Applied(()),
        OperationResult::Failed(failure) => Outcome::Failed {
            kind: failure.kind,
            detail: failure.detail,
        },
        OperationResult::Query(response) => Outcome::Failed {
            kind: FailureKind::Internal,
            detail: format!("{request_name} 收到查询应答 {response:?}"),
        },
    }
}

/// 从查询应答里取载荷;形状不符 / 失败都收敛成结构化结论。
pub(crate) fn decode_query<T>(
    result: OperationResult,
    request_name: &'static str,
    extract: impl FnOnce(mineral_protocol::Response) -> Option<T>,
) -> Outcome<T> {
    match result {
        OperationResult::Query(response) => match extract(*response) {
            Some(value) => Outcome::Applied(value),
            None => Outcome::Failed {
                kind: FailureKind::Internal,
                detail: format!("{request_name} 收到意外应答"),
            },
        },
        OperationResult::Failed(failure) => Outcome::Failed {
            kind: failure.kind,
            detail: failure.detail,
        },
        OperationResult::Applied | OperationResult::Accepted => Outcome::Failed {
            kind: FailureKind::Internal,
            detail: format!("{request_name} 缺少查询载荷"),
        },
    }
}

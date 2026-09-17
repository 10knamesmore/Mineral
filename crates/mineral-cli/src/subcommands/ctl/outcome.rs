//! `ctl` 命令的结论模型:会话结论的四种取值,加 CLI 本地判定的「跳过」。
//!
//! `skipped` 只可能由 CLI 产生(没有当前曲、时长未知、队列无变化),会话层看不到它;
//! 失败类别则既可能是 daemon 给出的结构化类别,也可能是 CLI 本地判定。

use mineral_client::operation::Outcome;
use mineral_protocol::FailureKind;

/// 一条命令的结论态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum State {
    /// daemon 已应用。
    Applied,

    /// 后台工作已受理(完成由订阅状态表达)。
    Accepted,

    /// CLI 判定这条命令不该发:没有可操作的对象。
    Skipped(SkipReason),

    /// 明确失败:daemon 的结构化结论,或 CLI 本地判定。
    Failed {
        /// 失败类别。
        reason: FailureReason,

        /// daemon 给出的人读原因(CLI 判定时为空)。
        detail: Option<String>,
    },

    /// 结果未知:未提交 / 连接丢失 / 等状态超时。
    Unknown {
        /// 人读原因。
        detail: String,
    },
}

impl State {
    /// JSON `outcome` 字段值。
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Accepted => "accepted",
            Self::Skipped(_) => "skipped",
            Self::Failed { .. } => "failed",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// 是否算成功(退出码 0)。
    pub(super) fn is_success(&self) -> bool {
        matches!(self, Self::Applied | Self::Accepted | Self::Skipped(_))
    }
}

/// CLI 判定这条命令没有可操作对象时的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkipReason {
    /// 没有当前曲,切换方向无从谈起。
    NoCurrentTrack,

    /// 时长未知,相对跳没有 clamp 依据。
    DurationUnknown,

    /// 队列没有变化(撤销无历史、变换返回原序)。
    NoChange,
}

impl SkipReason {
    /// JSON `reason` 字段值。
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NoCurrentTrack => "no-current-track",
            Self::DurationUnknown => "duration-unknown",
            Self::NoChange => "no-change",
        }
    }

    /// 人读说明。
    pub(super) fn human(self) -> &'static str {
        match self {
            Self::NoCurrentTrack => "没有当前曲",
            Self::DurationUnknown => "时长未知,无法相对跳转",
            Self::NoChange => "队列没有变化",
        }
    }
}

/// 失败类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureReason {
    /// daemon 判定。
    Daemon(FailureKind),

    /// 队列编辑锚点过期:client 视图与 daemon 队列不一致。
    Stale,

    /// 有效配置里没有这个队列变换 label。
    UnknownTransform,

    /// 有效配置里有多条同名队列变换 label。
    AmbiguousTransform,
}

impl FailureReason {
    /// JSON `kind` 字段值。
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Daemon(kind) => match kind {
                FailureKind::Invalid => "invalid",
                FailureKind::NotFound => "not-found",
                FailureKind::Unavailable => "unavailable",
                FailureKind::Conflict => "conflict",
                FailureKind::Internal => "internal",
            },
            Self::Stale => "stale",
            Self::UnknownTransform => "unknown-transform",
            Self::AmbiguousTransform => "ambiguous-transform",
        }
    }

    /// 人读说明。
    pub(super) fn human(self) -> &'static str {
        match self {
            Self::Daemon(FailureKind::Invalid) => "请求参数不合法",
            Self::Daemon(FailureKind::NotFound) => "目标不存在",
            Self::Daemon(FailureKind::Unavailable) => "依赖能力当前不可用",
            Self::Daemon(FailureKind::Conflict) => "与当前状态冲突",
            Self::Daemon(FailureKind::Internal) => "daemon 内部错误",
            Self::Stale => "队列已被改动或变换返回了队列外的 id,本次未执行",
            Self::UnknownTransform => "有效配置里没有这个队列变换",
            Self::AmbiguousTransform => "有效配置里有多个同名队列变换",
        }
    }
}

/// 把会话结论翻成 CLI 状态。
///
/// # Params:
///   - `outcome`: 一次控制请求的会话结论
pub(super) fn state_from_outcome(outcome: Outcome<()>) -> State {
    match outcome {
        Outcome::Applied(()) => State::Applied,
        Outcome::Accepted(()) => State::Accepted,
        Outcome::Failed { kind, detail } => failed_state(kind, detail),
        Outcome::Unknown { detail } => unknown_state(detail),
    }
}

/// daemon 结构化失败的结论态。
///
/// # Params:
///   - `kind`: daemon 给出的失败类别
///   - `detail`: daemon 给出的人读原因(空串不落成空 detail)
pub(super) fn failed_state(kind: FailureKind, detail: String) -> State {
    State::Failed {
        reason: FailureReason::Daemon(kind),
        detail: (!detail.is_empty()).then_some(detail),
    }
}

/// 结果未知的结论态。
///
/// # Params:
///   - `detail`: 人读原因
pub(super) fn unknown_state(detail: String) -> State {
    State::Unknown { detail }
}

#[cfg(test)]
mod tests {
    use mineral_client::operation::Outcome;
    use mineral_protocol::FailureKind;

    use super::{FailureReason, SkipReason, State, state_from_outcome};

    /// 会话结论按类翻到 CLI 状态,失败保留 daemon 的结构化类别。
    #[test]
    fn outcome_maps_to_state() {
        assert_eq!(state_from_outcome(Outcome::Applied(())), State::Applied);
        assert_eq!(state_from_outcome(Outcome::Accepted(())), State::Accepted);
        assert_eq!(
            state_from_outcome(Outcome::Failed {
                kind: FailureKind::Invalid,
                detail: "空队列".to_owned(),
            }),
            State::Failed {
                reason: FailureReason::Daemon(FailureKind::Invalid),
                detail: Some("空队列".to_owned()),
            }
        );
        assert_eq!(
            state_from_outcome(Outcome::Unknown {
                detail: "未提交".to_owned(),
            }),
            State::Unknown {
                detail: "未提交".to_owned(),
            }
        );
    }

    /// 空 detail 不落成空串,免得人读输出尾随一个破折号。
    #[test]
    fn empty_failure_detail_is_dropped() {
        assert_eq!(
            state_from_outcome(Outcome::Failed {
                kind: FailureKind::Internal,
                detail: String::new(),
            }),
            State::Failed {
                reason: FailureReason::Daemon(FailureKind::Internal),
                detail: None,
            }
        );
    }

    /// 只有成功与跳过算成功;失败与未知都不是。
    #[test]
    fn only_success_and_skip_are_success() {
        assert!(State::Applied.is_success());
        assert!(State::Accepted.is_success());
        assert!(State::Skipped(SkipReason::NoChange).is_success());
        assert!(
            !State::Failed {
                reason: FailureReason::Stale,
                detail: None,
            }
            .is_success()
        );
        assert!(
            !State::Unknown {
                detail: "断线".to_owned(),
            }
            .is_success()
        );
    }

    /// JSON 与 `kind` 字段取值是稳定契约。
    #[test]
    fn state_and_reason_names_are_stable() {
        assert_eq!(State::Applied.as_str(), "applied");
        assert_eq!(State::Accepted.as_str(), "accepted");
        assert_eq!(
            State::Skipped(SkipReason::NoCurrentTrack).as_str(),
            "skipped"
        );
        assert_eq!(
            State::Failed {
                reason: FailureReason::Daemon(FailureKind::Conflict),
                detail: None,
            }
            .as_str(),
            "failed"
        );
        assert_eq!(SkipReason::NoCurrentTrack.as_str(), "no-current-track");
        assert_eq!(SkipReason::DurationUnknown.as_str(), "duration-unknown");
        assert_eq!(SkipReason::NoChange.as_str(), "no-change");
        assert_eq!(FailureReason::Stale.as_str(), "stale");
        assert_eq!(
            FailureReason::AmbiguousTransform.as_str(),
            "ambiguous-transform"
        );
        assert_eq!(
            FailureReason::Daemon(FailureKind::Unavailable).as_str(),
            "unavailable"
        );
    }
}

//! `ctl` 结论的两种呈现:人读一行文本,或 `--json` 一行信封。
//!
//! 两种输出由同一份 [`Report`] 渲染,人读文案按结构化结论生成。JSON 字段名与取值是脚本
//! 契约:只增不改名(见 Spec「Shared implementation decisions」)。

use std::process::ExitCode;

use serde::Serialize;

use crate::error::{Error, Result};

use super::outcome::{FailureReason, State};

/// 命令专属的结构化字段(同一时刻至多一个)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Payload {
    /// 新建歌单的完整身份。
    PlaylistId(String),

    /// `volume` 的目标百分比。
    VolumePct(u8),

    /// `seek` 的目标位置(ms)。
    PositionMs(u64),
}

/// 一条命令的输出:结构化结论 + 人读摘要。
#[derive(Debug)]
pub(super) struct Report {
    /// JSON `command` 字段值(子命令路径,如 `queue transform`)。
    command: &'static str,

    /// 结论态。
    state: State,

    /// 成功时的人读摘要(如 `paused`);非成功态不用。
    summary: String,

    /// 命令专属字段。
    payload: Option<Payload>,
}

impl Report {
    /// 组装一条命令结论。
    ///
    /// # Params:
    ///   - `command`: 子命令路径
    ///   - `summary`: 成功时的人读摘要
    ///   - `state`: 结论态
    pub(super) fn new(command: &'static str, summary: impl Into<String>, state: State) -> Self {
        Self {
            command,
            state,
            summary: summary.into(),
            payload: None,
        }
    }

    /// 「没执行」的结论(连不上 daemon、握手失败)。
    ///
    /// # Params:
    ///   - `command`: 子命令路径
    ///   - `detail`: 人读原因
    pub(super) fn not_executed(command: &'static str, detail: String) -> Self {
        Self::new(command, String::new(), State::Unknown { detail })
    }

    /// CLI 本地判定的失败(如 daemon 能力中找不到队列变换名称)。
    ///
    /// # Params:
    ///   - `command`: 子命令路径
    ///   - `reason`: 失败类别
    ///   - `detail`: 人读补充信息
    pub(super) fn failed(
        command: &'static str,
        reason: FailureReason,
        detail: Option<String>,
    ) -> Self {
        Self::new(command, String::new(), State::Failed { reason, detail })
    }

    /// 附上命令专属字段。
    ///
    /// # Params:
    ///   - `payload`: 命令专属字段
    pub(super) fn with_payload(mut self, payload: Payload) -> Self {
        self.payload = Some(payload);
        self
    }

    /// 输出结论并给出进程退出码:成功(含跳过)0、被拒 1、没执行 3。
    ///
    /// # Params:
    ///   - `json`: 是否输出一行 JSON 信封(否则人读一行)
    ///
    /// # Return:
    ///   进程退出码;JSON 渲染失败时冒泡(本地程序错误,不是命令结论)。
    pub(super) fn emit(self, json: bool) -> Result<ExitCode> {
        let code = self.exit_code();
        if json {
            let line = self.json_line().map_err(|source| Error::Json {
                output: "ctl outcome",
                source,
            })?;
            println!("{line}");
            return Ok(code);
        }
        let (stdout, stderr) = self.human_lines();
        if let Some(line) = stdout {
            println!("{line}");
        }
        if let Some(line) = stderr {
            eprintln!("{line}");
        }
        Ok(code)
    }

    /// 退出码:成功 0、被拒 1、没执行 3(clap 参数错误另有 2,到不了这里)。
    fn exit_code(&self) -> ExitCode {
        if self.state.is_success() {
            ExitCode::SUCCESS
        } else if matches!(self.state, State::Failed { .. }) {
            ExitCode::from(1)
        } else {
            ExitCode::from(3)
        }
    }

    /// 人读输出:成功与跳过走 stdout,失败与未知走 stderr。
    fn human_lines(&self) -> (Option<String>, Option<String>) {
        match &self.state {
            State::Applied | State::Accepted => (Some(self.summary.clone()), None),
            State::Skipped(reason) => (Some(format!("skipped: {}", reason.human())), None),
            State::Failed { reason, detail } => {
                let line = match detail {
                    Some(detail) => format!("failed: {} — {detail}", reason.human()),
                    None => format!("failed: {}", reason.human()),
                };
                (None, Some(line))
            }
            State::Unknown { detail } => (None, Some(format!("unknown: {detail}"))),
        }
    }

    /// 一行 JSON 信封。
    fn json_line(&self) -> std::result::Result<String, serde_json::Error> {
        let (kind, reason, detail) = match &self.state {
            State::Failed { reason, detail } => (Some(reason.as_str()), None, detail.as_deref()),
            State::Skipped(reason) => (None, Some(reason.as_str()), None),
            State::Unknown { detail } => (None, None, Some(detail.as_str())),
            State::Applied | State::Accepted => (None, None, None),
        };
        let (volume_pct, position_ms, playlist_id) = match &self.payload {
            Some(Payload::VolumePct(pct)) => (Some(*pct), None, None),
            Some(Payload::PositionMs(ms)) => (None, Some(*ms), None),
            Some(Payload::PlaylistId(id)) => (None, None, Some(id.as_str())),
            None => (None, None, None),
        };
        let envelope = Envelope {
            command: self.command,
            outcome: self.state.as_str(),
            kind,
            reason,
            detail,
            volume_pct,
            position_ms,
            playlist_id,
        };
        serde_json::to_string(&envelope)
    }
}

/// 一行 JSON 信封的字段。
#[derive(Serialize)]
struct Envelope<'a> {
    /// 保存成功的歌单身份。
    #[serde(skip_serializing_if = "Option::is_none")]
    playlist_id: Option<&'a str>,

    /// 子命令路径。
    command: &'a str,

    /// 结论态。
    outcome: &'a str,

    /// 失败类别(仅失败时出现)。
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,

    /// 跳过原因(仅跳过时出现)。
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,

    /// CLI 生成的补充信息(仅失败 / 未知时出现)。
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,

    /// `volume` 的目标百分比。
    #[serde(skip_serializing_if = "Option::is_none")]
    volume_pct: Option<u8>,

    /// `seek` 的目标位置(ms)。
    #[serde(skip_serializing_if = "Option::is_none")]
    position_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::{Payload, Report};
    use crate::subcommands::ctl::outcome::{FailureReason, SkipReason, State, state_from_outcome};
    use mineral_client::operation::{Outcome, SubmitError, UnknownReason};
    use mineral_protocol::FailureKind;

    /// JSON 信封的字段名 / 顺序 / 取值集合是脚本契约,变更必须显式审阅。
    #[test]
    fn json_contract_snapshot() -> color_eyre::Result<()> {
        let lines = [
            Report::new("pause", "paused", State::Applied),
            Report::new("volume", "volume 45%", State::Applied)
                .with_payload(Payload::VolumePct(45)),
            Report::new("seek", "seek 1:30", State::Applied)
                .with_payload(Payload::PositionMs(90_000)),
            Report::new(
                "play-pause",
                String::new(),
                State::Skipped(SkipReason::NoCurrentTrack),
            ),
            Report::new(
                "queue transform",
                String::new(),
                State::Failed {
                    reason: FailureReason::UnknownTransform,
                    detail: None,
                },
            ),
            Report::not_executed("pause", "连不上 daemon".to_owned()),
        ]
        .into_iter()
        .map(|report| report.json_line())
        .collect::<Result<Vec<String>, serde_json::Error>>()?
        .join("\n");
        mineral_test::assert_snap!(
            "ctl --json 信封:核心字段 + 命令专属字段 + 四种结论态",
            lines
        );
        Ok(())
    }

    /// daemon 失败保留 JSON 分类，但不把诊断原文放入机器信封。
    #[test]
    fn daemon_failure_json_omits_internal_detail() -> color_eyre::Result<()> {
        let state = state_from_outcome(Outcome::Failed {
            kind: FailureKind::Internal,
            detail: "internal diagnostic".to_owned(),
        });
        let line = Report::new("pause", String::new(), state).json_line()?;
        let json = serde_json::from_str::<serde_json::Value>(&line)?;
        assert_eq!(
            json.get("outcome").and_then(serde_json::Value::as_str),
            Some("failed")
        );
        assert_eq!(
            json.get("kind").and_then(serde_json::Value::as_str),
            Some("internal")
        );
        assert!(json.get("detail").is_none());
        Ok(())
    }

    /// 未提交仍按 unknown 输出，原因说明由 CLI 生成且占用既有 detail 字段。
    #[test]
    fn not_submitted_json_keeps_unknown_envelope() -> color_eyre::Result<()> {
        let state = state_from_outcome(Outcome::Unknown {
            reason: UnknownReason::NotSubmitted(SubmitError::Disconnected),
        });
        let line = Report::new("pause", String::new(), state).json_line()?;
        let json = serde_json::from_str::<serde_json::Value>(&line)?;
        assert_eq!(
            json.get("outcome").and_then(serde_json::Value::as_str),
            Some("unknown")
        );
        assert!(json.get("detail").is_some_and(serde_json::Value::is_string));
        Ok(())
    }
}

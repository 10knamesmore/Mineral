//! `ctl` 子命令的执行:连接 daemon、按命令分发、输出结论。
//!
//! 命令面与参数解析见 [`super::command`],结论模型与渲染见 [`super::outcome`] / [`super::render`]。

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use color_eyre::eyre::WrapErr as _;
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
use mineral_protocol::{
    Event, QueueEditOutcome, QueueOp, SocketWire, Subscription, SubscriptionId, SubscriptionTopic,
};

use super::command::{CtlArgs, CtlCommand, QueueCommand, SeekSpec, VolumeSpec, format_position};
use super::outcome::{
    FailureReason, SkipReason, State, failed_state, state_from_outcome, unknown_state,
};
use super::render::{Payload, Report};

/// 等订阅首帧的上限(daemon 订阅即推,正常毫秒级;`mineral status` 用同一量级)。
const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// 执行 `mineral ctl` 命令。
///
/// # Params:
///   - `args`: 已解析的 `ctl` 参数(含 `--json`)
///
/// # Return:
///   进程退出码:成功 0、被拒 1、没执行 3。
pub async fn run(args: CtlArgs) -> color_eyre::Result<ExitCode> {
    let socket_path = mineral_paths::socket_path().wrap_err("解析 daemon socket 路径失败")?;
    let report = execute(&socket_path, args.cmd).await;
    report.emit(args.json)
}

/// 连 daemon 并执行一条命令;连不上 / 握手失败产出「没执行」结论,不冒泡成 CLI 错误。
///
/// # Params:
///   - `socket_path`: daemon socket 路径
///   - `cmd`: 已解析的控制命令
async fn execute(socket_path: &Path, cmd: CtlCommand) -> Report {
    let client = match connect(socket_path).await {
        Ok(client) => client,
        Err(detail) => return Report::not_executed(cmd.path(), detail),
    };
    dispatch(&client, cmd).await
}

/// 按命令分发:不需要当前状态的直接提交,需要的先等订阅就绪。
///
/// # Params:
///   - `client`: 已连接的会话 client
///   - `cmd`: 已解析的控制命令
async fn dispatch(client: &Client, cmd: CtlCommand) -> Report {
    let path = cmd.path();
    match cmd {
        CtlCommand::PlayPause => play_pause(client, path).await,
        CtlCommand::Pause => report(path, "paused", client.pause().await),
        CtlCommand::Resume => report(path, "resumed", client.resume().await),
        CtlCommand::Stop => report(path, "stopped", client.stop().await),
        CtlCommand::Next => report(path, "next", client.next_song().await),
        CtlCommand::Prev => report(path, "prev", client.prev_or_restart().await),
        CtlCommand::Mode { mode: None } => {
            report(path, "mode: cycled", client.cycle_play_mode().await)
        }
        CtlCommand::Mode { mode: Some(mode) } => {
            let label = mode.label();
            report(
                path,
                format!("mode: {label}"),
                client.set_play_mode(mode.into()).await,
            )
        }
        CtlCommand::Seek { target } => match target {
            SeekSpec::Absolute(position_ms) => seek_absolute(client, path, position_ms).await,
            SeekSpec::Relative(delta_ms) => seek_relative(client, path, delta_ms).await,
        },
        CtlCommand::Volume { target } => match target {
            VolumeSpec::Absolute(pct) => volume_absolute(client, path, pct).await,
            VolumeSpec::Relative(delta) => volume_relative(client, path, delta).await,
        },
        CtlCommand::Queue { cmd } => dispatch_queue(client, cmd).await,
    }
}

/// 绝对跳转:直接提交命令行给的目标。
///
/// # Params:
///   - `client`: 会话 client
///   - `path`: 子命令路径
///   - `position_ms`: 目标位置(ms)
async fn seek_absolute(client: &Client, path: &'static str, position_ms: u64) -> Report {
    let summary = format!("seek {}", format_position(position_ms));
    report(path, summary, client.seek(position_ms).await)
        .with_payload(Payload::PositionMs(position_ms))
}

/// 绝对音量:直接提交命令行给的目标。
///
/// # Params:
///   - `client`: 会话 client
///   - `path`: 子命令路径
///   - `pct`: 目标百分比
async fn volume_absolute(client: &Client, path: &'static str, pct: u8) -> Report {
    report(path, format!("volume {pct}%"), client.set_volume(pct).await)
        .with_payload(Payload::VolumePct(pct))
}

/// `play-pause`:按当前曲与在播状态选方向;没有当前曲时不发请求。
///
/// # Params:
///   - `client`: 会话 client
///   - `path`: 子命令路径
async fn play_pause(client: &Client, path: &'static str) -> Report {
    if let Err(detail) = await_topic(client, SubscriptionTopic::Player).await {
        return Report::not_executed(path, detail);
    }
    if let Err(detail) = await_topic(client, SubscriptionTopic::Playback).await {
        return Report::not_executed(path, detail);
    }
    let has_current_song = client.mirror().read_player(|player| {
        player
            .current()
            .is_some_and(|current| current.current_song.is_some())
    });
    if !has_current_song {
        return Report::new(
            path,
            String::new(),
            State::Skipped(SkipReason::NoCurrentTrack),
        );
    }
    let playing = client
        .mirror()
        .read_playback(|playback| playback.anchor().playing);
    if playing {
        report(path, "paused", client.pause().await)
    } else {
        report(path, "resumed", client.resume().await)
    }
}

/// 相对跳:按当前锚点的时长钳位;时长未知时不猜 clamp 依据。
///
/// # Params:
///   - `client`: 会话 client
///   - `path`: 子命令路径
///   - `delta_ms`: 相对偏移(ms)
async fn seek_relative(client: &Client, path: &'static str, delta_ms: i64) -> Report {
    if let Err(detail) = await_topic(client, SubscriptionTopic::Playback).await {
        return Report::not_executed(path, detail);
    }
    let (anchor, position_ms) = client.mirror().read_playback(|playback| {
        (
            playback.anchor().clone(),
            playback.position_ms(Instant::now()),
        )
    });
    let Some(duration_ms) = anchor.duration_ms else {
        return Report::new(
            path,
            String::new(),
            State::Skipped(SkipReason::DurationUnknown),
        );
    };
    let target_ms = clamp_position(position_ms, delta_ms, duration_ms);
    let summary = format!("seek {}", format_position(target_ms));
    report(path, summary, client.seek(target_ms).await).with_payload(Payload::PositionMs(target_ms))
}

/// 相对音量:按当前锚点的百分比增减,结果钳在 0..=100。
///
/// # Params:
///   - `client`: 会话 client
///   - `path`: 子命令路径
///   - `delta`: 相对增量(百分点)
async fn volume_relative(client: &Client, path: &'static str, delta: i16) -> Report {
    if let Err(detail) = await_topic(client, SubscriptionTopic::Playback).await {
        return Report::not_executed(path, detail);
    }
    let current_pct = client
        .mirror()
        .read_playback(|playback| playback.anchor().volume_pct);
    let target_pct = clamp_volume(current_pct, delta);
    report(
        path,
        format!("volume {target_pct}%"),
        client.set_volume(target_pct).await,
    )
    .with_payload(Payload::VolumePct(target_pct))
}

/// `ctl queue` 子命令分发。
///
/// # Params:
///   - `client`: 会话 client
///   - `cmd`: 队列子命令
async fn dispatch_queue(client: &Client, cmd: QueueCommand) -> Report {
    let path = cmd.path();
    match cmd {
        QueueCommand::Transform { label, at } => queue_transform(client, path, &label, at).await,
        QueueCommand::Undo => queue_edit_report(path, client.queue_edit(QueueOp::Undo).await),
    }
}

/// 具名队列变换:从 daemon 推送的有效配置解析 label,提交一次 `ApplyTransform`。
///
/// label 必须唯一命中:没有这个 label 或多条同名都明确失败,不静默取第一个。
///
/// # Params:
///   - `client`: 会话 client
///   - `path`: 子命令路径
///   - `label`: 有效配置里的变换 label
///   - `at`: 变换的光标上下文(0-based 队列下标)
async fn queue_transform(
    client: &Client,
    path: &'static str,
    label: &str,
    at: Option<usize>,
) -> Report {
    if let Err(detail) = await_topic(client, SubscriptionTopic::Events(Subscription::Config)).await
    {
        return Report::not_executed(path, detail);
    }
    let Some(tree) = latest_config(client) else {
        return Report::not_executed(path, "没收到 daemon 的有效配置".to_owned());
    };
    let config = match mineral_config::from_tree(&tree.into_json()) {
        Ok(config) => config,
        Err(warning) => {
            return Report::not_executed(path, format!("有效配置落型失败:{warning}"));
        }
    };
    let labels = config
        .queue()
        .transforms()
        .iter()
        .map(|spec| spec.label().clone())
        .collect::<Vec<String>>();
    let index = match transform_index(&labels, label) {
        TransformLookup::Found(index) => index,
        TransformLookup::Unknown => {
            return Report::failed(
                path,
                FailureReason::UnknownTransform,
                Some(available_detail(&labels)),
            );
        }
        TransformLookup::Ambiguous => {
            return Report::failed(
                path,
                FailureReason::AmbiguousTransform,
                Some(duplicate_detail(&labels, label)),
            );
        }
    };
    queue_edit_report(
        path,
        client
            .queue_edit(QueueOp::ApplyTransform {
                index,
                selected: at,
            })
            .await,
    )
}

/// 把队列编辑结论翻成 CLI 结论:`Applied` → applied、`NoOp` → skipped、`Stale` → failed。
///
/// `Stale` 表示 client 视图与 daemon 队列不一致,不自动重试。
///
/// # Params:
///   - `path`: 子命令路径
///   - `outcome`: 一次队列编辑的会话结论
fn queue_edit_report(path: &'static str, outcome: Outcome<QueueEditOutcome>) -> Report {
    match outcome {
        Outcome::Applied(edit) | Outcome::Accepted(edit) => match edit {
            QueueEditOutcome::Applied => Report::new(path, "queue updated", State::Applied),
            QueueEditOutcome::NoOp => {
                Report::new(path, String::new(), State::Skipped(SkipReason::NoChange))
            }
            QueueEditOutcome::Stale => Report::new(
                path,
                String::new(),
                State::Failed {
                    reason: FailureReason::Stale,
                    detail: None,
                },
            ),
        },
        Outcome::Failed { kind, detail } => {
            Report::new(path, String::new(), failed_state(kind, detail))
        }
        Outcome::Unknown { detail } => Report::new(path, String::new(), unknown_state(detail)),
    }
}

/// 等 `topic` 的首帧到达。
///
/// 连接成功不等于订阅首帧已到,所以等待之后必须用 `subscription_seen` 确认;未就绪即返回人读
/// 原因,调用方据此产出「没执行」,不拿镜像初始值当 daemon 状态。
///
/// # Params:
///   - `client`: 会话 client
///   - `topic`: 要等首帧的订阅主题
///
/// # Return:
///   订阅 id;超时或未就绪时返回人读原因。
async fn await_topic(client: &Client, topic: SubscriptionTopic) -> Result<SubscriptionId, String> {
    let id = client.subscribe(topic);
    client.wait_subscriptions_ready(READY_TIMEOUT).await;
    if client.mirror().subscription_seen(id) {
        Ok(id)
    } else {
        Err(format!("等 {topic:?} 首帧超时"))
    }
}

/// 取事件流里最后一条有效配置树(订阅 Config 时握手后会先收到一帧)。
///
/// # Params:
///   - `client`: 会话 client
fn latest_config(client: &Client) -> Option<mineral_protocol::BusValue> {
    client
        .mirror()
        .drain_events()
        .into_iter()
        .rev()
        .find_map(|event| match event {
            Event::ConfigChanged { config } => Some(config),
            _ => None,
        })
}

/// 按 label 在有效配置的变换表里定位下标。
///
/// # Params:
///   - `labels`: 有效配置里的变换 label(数组序即下标)
///   - `want`: 命令行给的 label
fn transform_index(labels: &[String], want: &str) -> TransformLookup {
    let mut found = None;
    for (index, label) in labels.iter().enumerate() {
        if label != want {
            continue;
        }
        if found.is_some() {
            return TransformLookup::Ambiguous;
        }
        found = Some(index);
    }
    found.map_or(TransformLookup::Unknown, TransformLookup::Found)
}

/// 按 label 定位变换下标的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformLookup {
    /// 恰好一条命中。
    Found(usize),

    /// 有效配置里没有这个 label。
    Unknown,

    /// 有效配置里有多条同名 label。
    Ambiguous,
}

/// 未命中时的补充信息:列出可用 label。
///
/// # Params:
///   - `labels`: 有效配置里的变换 label
fn available_detail(labels: &[String]) -> String {
    if labels.is_empty() {
        return "有效配置里没有注册任何队列变换".to_owned();
    }
    format!("可用变换:{}", labels.join(", "))
}

/// 同名命中时的补充信息:列出冲突下标。
///
/// # Params:
///   - `labels`: 有效配置里的变换 label
///   - `want`: 命令行给的 label
fn duplicate_detail(labels: &[String], want: &str) -> String {
    let indices = labels
        .iter()
        .enumerate()
        .filter(|(_, label)| label.as_str() == want)
        .map(|(index, _)| index.to_string())
        .collect::<Vec<String>>();
    format!("同名 label `{want}` 出现在下标 {}", indices.join(", "))
}

/// 「当前位置 + 偏移」钳到 `[0, duration]`。
///
/// # Params:
///   - `current_ms`: 当前位置(ms)
///   - `delta_ms`: 偏移(ms,负数为回退)
///   - `duration_ms`: 当前曲时长(ms)
fn clamp_position(current_ms: u64, delta_ms: i64, duration_ms: u64) -> u64 {
    let distance = delta_ms.unsigned_abs();
    if delta_ms >= 0 {
        current_ms.saturating_add(distance).min(duration_ms)
    } else {
        current_ms.saturating_sub(distance)
    }
}

/// 当前音量加增量,钳在 0..=100。
///
/// # Params:
///   - `current_pct`: 当前音量
///   - `delta`: 增量(百分点)
fn clamp_volume(current_pct: u8, delta: i16) -> u8 {
    let sum = i16::from(current_pct).saturating_add(delta).clamp(0, 100);
    u8::try_from(sum).unwrap_or(current_pct)
}

/// 组装一条控制请求的结论。
///
/// # Params:
///   - `command`: 子命令路径
///   - `summary`: 成功时的人读摘要
///   - `outcome`: 会话结论
fn report(command: &'static str, summary: impl Into<String>, outcome: Outcome<()>) -> Report {
    Report::new(command, summary, state_from_outcome(outcome))
}

/// 连 daemon;失败原因交给调用者作为「没执行」结论输出。
///
/// # Params:
///   - `socket_path`: daemon socket 路径
///
/// # Return:
///   会话 client;连不上 / 握手被拒时返回人读原因。
async fn connect(socket_path: &Path) -> Result<Client, String> {
    let wire = SocketWire::connect(socket_path)
        .await
        .map_err(|error| format!("连不上 daemon({error});先跑 `mineral serve`"))?;
    Client::from_wire(Box::new(wire), "mineral_ctl", ClientConfig::cli())
        .await
        .map_err(|error| format!("与 daemon 握手失败:{error}"))
}

#[cfg(test)]
mod tests {
    use super::{clamp_position, clamp_volume};

    /// 相对跳钳在 `[0, duration]`。
    #[test]
    fn position_clamps_to_duration() {
        assert_eq!(clamp_position(90_000, 10_000, 95_000), 95_000, "越过末尾");
        assert_eq!(clamp_position(5_000, -10_000, 95_000), 0, "越过开头");
        assert_eq!(clamp_position(30_000, -10_000, 95_000), 20_000, "正常回退");
        assert_eq!(clamp_position(30_000, 10_000, 95_000), 40_000, "正常前进");
    }

    /// 相对音量钳在 0..=100,越界用端点而不是回卷。
    #[test]
    fn volume_clamps_to_range() {
        assert_eq!(clamp_volume(98, 5), 100);
        assert_eq!(clamp_volume(2, -5), 0);
        assert_eq!(clamp_volume(50, 5), 55);
        assert_eq!(clamp_volume(50, -5), 45);
        assert_eq!(clamp_volume(50, i16::MAX), 100, "荒谬增量也落在端点");
    }
}

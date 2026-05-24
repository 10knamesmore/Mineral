//! IPC accept loop + 单 connection dispatch。
//!
//! 当前实现限制:**单 client**——已有 connection 时,后续 incoming connect
//! 立刻收到一个 [`Response::Error`] 然后被关掉。多 client / fanout 留 4c。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::eyre::WrapErr;
use mineral_protocol::{Request, Response, framed, recv, send};
use tokio::net::{UnixListener, UnixStream};

use crate::client::{Client, ClientHandle};

/// Accept loop。返回 `Ok(())` 仅在 listener 被外部关闭时;否则一直循环。
///
/// `on_connect` 在每条新 connection 被接受后立刻调用一次,调用方借此重新触发
/// 「初始数据加载」(`MyPlaylists` / `LikedSongIds` 等)——必要的:`drain_task_events`
/// 是消费式语义,首个 client 拿走 events 后 buffer 清空,新 client 看不到任何
/// 历史 event 会显示「数据为空」假象。dedup 命中既存任务时无副作用。
pub(crate) async fn run<F>(
    listener: UnixListener,
    client: ClientHandle,
    busy: Arc<AtomicBool>,
    on_connect: F,
) -> color_eyre::Result<()>
where
    F: Fn() + Send + Sync + 'static,
{
    let on_connect = Arc::new(on_connect);
    loop {
        let (stream, _addr) = listener
            .accept()
            .await
            .wrap_err("UnixListener::accept failed")?;
        if busy.swap(true, Ordering::AcqRel) {
            // 已经有 client 在用 → 立刻拒绝。失败也无所谓,client 自己会 EOF。
            mineral_log::warn!(target: "ipc", "rejected new connection: single-client busy");
            tokio::spawn(reject_busy(stream));
            continue;
        }
        on_connect();
        mineral_log::info!(target: "ipc", "client connected");
        let client = client.clone();
        let busy_clone = Arc::clone(&busy);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, &client).await {
                mineral_log::warn!(target: "ipc", error = mineral_log::chain(&e), "connection ended with error");
            }
            busy_clone.store(false, Ordering::Release);
        });
    }
}

/// 已有 client 在用,新连进来的 client 收一条 [`Response::Error`] 后被关掉。
/// 失败也无所谓 —— 对方 socket 自己会 EOF。
async fn reject_busy(stream: UnixStream) {
    let mut framed = framed(stream);
    let _ = send(
        &mut framed,
        &Response::Error("daemon busy: another client is connected".to_owned()),
    )
    .await;
}

/// 接管已 accept 的 connection,跑 req-resp 循环直到 client EOF 或写失败。
/// 协议是严格 1:1 顺序的(每条 [`Request`] 必有一条 [`Response`])。
async fn handle_connection(stream: UnixStream, client: &ClientHandle) -> color_eyre::Result<()> {
    let mut framed = framed(stream);
    while let Some(req) = recv::<Request, _>(&mut framed).await? {
        let resp = dispatch(req, client);
        send(&mut framed, &resp).await?;
    }
    Ok(())
}

/// [`Request`] 到 [`Response`] 的纯函数 dispatch:每条 variant 对应一个 [`Client`]
/// trait 方法调用,组装成预期的 Response variant。所有错误兜底走 trait 实现层
/// (出错时返回值类用「合理默认值」),这里不应该再产生 [`Response::Error`]。
fn dispatch(req: Request, client: &ClientHandle) -> Response {
    if let Some(name) = req_log_name(&req) {
        mineral_log::debug!(target: "ipc", request = name, "handle request");
    }
    match req {
        Request::Play(url) => {
            client.play(url);
            Response::Ok
        }
        Request::Pause => {
            client.pause();
            Response::Ok
        }
        Request::Resume => {
            client.resume();
            Response::Ok
        }
        Request::Stop => {
            client.stop();
            Response::Ok
        }
        Request::Seek(ms) => {
            client.seek(ms);
            Response::Ok
        }
        Request::SetVolume(pct) => {
            client.set_volume(pct);
            Response::Ok
        }
        Request::AudioSnapshot => Response::AudioSnapshot(client.audio_snapshot()),
        Request::SubmitTask(kind, priority) => Response::TaskId(client.submit_task(kind, priority)),
        Request::CancelTasks(filter) => {
            client.cancel_tasks(filter);
            Response::Ok
        }
        Request::DrainTaskEvents => Response::TaskEvents(client.drain_task_events()),
        Request::TaskSnapshot => Response::TaskSnapshot(client.task_snapshot()),
        Request::PlaySong(song) => {
            client.play_song(*song);
            Response::Ok
        }
        Request::SetQueue { queue, target_id } => {
            client.set_queue(queue, target_id);
            Response::Ok
        }
        Request::CyclePlayMode => {
            client.cycle_play_mode();
            Response::Ok
        }
        Request::PrevOrRestart => {
            client.prev_or_restart();
            Response::Ok
        }
        Request::NextSong => {
            client.next_song();
            Response::Ok
        }
        Request::PlayerSnapshot => Response::PlayerSnapshot(Box::new(client.player_snapshot())),
        Request::PullPcm(n) => {
            let (samples, sample_rate) = client.pull_pcm(n);
            Response::PcmData {
                samples,
                sample_rate,
            }
        }
        Request::DaemonInfo => Response::DaemonInfo {
            pid: std::process::id(),
        },
    }
}

/// 请求的日志名;高频轮询类(snapshot / pcm / drain,TUI 每 tick 拉)返回 `None`
/// 不记,避免刷屏。其余状态变更类返回 variant 名。
fn req_log_name(req: &Request) -> Option<&'static str> {
    match req {
        Request::AudioSnapshot
        | Request::PlayerSnapshot
        | Request::TaskSnapshot
        | Request::DrainTaskEvents
        | Request::PullPcm(_) => None,
        Request::Play(_) => Some("Play"),
        Request::Pause => Some("Pause"),
        Request::Resume => Some("Resume"),
        Request::Stop => Some("Stop"),
        Request::Seek(_) => Some("Seek"),
        Request::SetVolume(_) => Some("SetVolume"),
        Request::SubmitTask(..) => Some("SubmitTask"),
        Request::CancelTasks(_) => Some("CancelTasks"),
        Request::PlaySong(_) => Some("PlaySong"),
        Request::SetQueue { .. } => Some("SetQueue"),
        Request::CyclePlayMode => Some("CyclePlayMode"),
        Request::PrevOrRestart => Some("PrevOrRestart"),
        Request::NextSong => Some("NextSong"),
        Request::DaemonInfo => Some("DaemonInfo"),
    }
}

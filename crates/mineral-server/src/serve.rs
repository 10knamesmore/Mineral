//! IPC accept loop + 单 connection dispatch。
//!
//! 当前实现限制:**单 client**——已有 connection 时,后续 incoming connect
//! 立刻收到一个 [`Response::Error`] 然后被关掉。多 client / fanout 留 4c。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::eyre::WrapErr;
use mineral_protocol::{Request, Response, framed, recv, send};
use tokio::net::{UnixListener, UnixStream};

use crate::client::ClientHandle;

/// Accept loop。返回 `Ok(())` 仅在 listener 被外部关闭时;否则一直循环。
///
/// `on_connect` 在每条新 connection 被接受后立刻调用一次,调用方借此重新触发
/// 「初始数据加载」(`MyPlaylists` / `LikedSongIds` 等)——必要的:`drain_task_events`
/// 是消费式语义,首个 client 拿走 events 后 buffer 清空,新 client 看不到任何
/// 历史 event 会显示「数据为空」假象。dedup 命中既存任务时无副作用。
pub(crate) async fn run<F>(
    listener: UnixListener,
    client: ClientHandle,
    on_connect: F,
) -> color_eyre::Result<()>
where
    F: Fn() + Send + Sync + 'static,
{
    let busy = Arc::new(AtomicBool::new(false));
    let on_connect = Arc::new(on_connect);
    loop {
        let (stream, _addr) = listener
            .accept()
            .await
            .wrap_err("UnixListener::accept failed")?;
        if busy.swap(true, Ordering::AcqRel) {
            // 已经有 client 在用 → 立刻拒绝。失败也无所谓,client 自己会 EOF。
            tokio::spawn(reject_busy(stream));
            continue;
        }
        on_connect();
        let client = client.clone();
        let busy_clone = Arc::clone(&busy);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, &client).await {
                mineral_log::warn!(target: "ipc", "connection ended with error: {e}");
            }
            busy_clone.store(false, Ordering::Release);
        });
    }
}

async fn reject_busy(stream: UnixStream) {
    let mut framed = framed(stream);
    let _ = send(
        &mut framed,
        &Response::Error("daemon busy: another client is connected".to_owned()),
    )
    .await;
}

async fn handle_connection(stream: UnixStream, client: &ClientHandle) -> color_eyre::Result<()> {
    let mut framed = framed(stream);
    while let Some(req) = recv::<Request, _>(&mut framed).await? {
        let resp = dispatch(req, client);
        send(&mut framed, &resp).await?;
    }
    Ok(())
}

fn dispatch(req: Request, client: &ClientHandle) -> Response {
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
    }
}

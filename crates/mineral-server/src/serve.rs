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
pub(crate) async fn run(listener: UnixListener, client: ClientHandle) -> color_eyre::Result<()> {
    let busy = Arc::new(AtomicBool::new(false));
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

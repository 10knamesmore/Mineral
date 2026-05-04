//! TUI 端的远程 client:同进程接口,跨进程实现。
//!
//! 实现 [`mineral_server::Client`] trait,所有方法 sync 接口,内部走 unix socket。
//! 一条长寿命 tokio task 处理 framed I/O;sync 调用通过 std::sync::mpsc 阻塞
//! 等 reply。
//!
//! - **fire-and-forget 类**(play / pause / 等):仍然走 round-trip(协议层 server
//!   每条 Request 必有 Response),但调用方丢弃 Response。
//! - **返回值类**:正常拿 Response 解出来;出错时返回「合理默认值」(`AudioSnapshot::default`
//!   等),错误细节进 `mineral_log::warn`。

use std::path::Path;

use color_eyre::eyre::{WrapErr, eyre};
use mineral_audio::AudioSnapshot;
use mineral_model::MediaUrl;
use mineral_protocol::{CancelFilter, Framed, Request, Response, framed, recv, send};
use mineral_server::Client;
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};
use rustc_hash::FxHashMap;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

/// 一条等回复的请求。worker 收到后 framed.send → recv → reply_tx.send。
struct Pending {
    req: Request,
    reply_tx: std::sync::mpsc::Sender<Response>,
}

/// TUI 端的远程 client。`Clone` 通过持 [`mpsc::UnboundedSender`] 廉价。
pub struct RemoteClient {
    req_tx: mpsc::UnboundedSender<Pending>,
}

impl RemoteClient {
    /// 连 daemon socket,起后台 worker。caller 必须在 tokio runtime 里(`mineral-tui::run`
    /// 是 async fn,自然满足)。
    pub async fn connect(socket_path: &Path) -> color_eyre::Result<Self> {
        let stream = UnixStream::connect(socket_path).await.wrap_err_with(|| {
            format!(
                "connect daemon socket {} (run `mineral serve` first?)",
                socket_path.display()
            )
        })?;
        let conn = framed(stream);
        let (req_tx, req_rx) = mpsc::unbounded_channel::<Pending>();
        tokio::spawn(worker(conn, req_rx));
        Ok(Self { req_tx })
    }

    /// 发请求 + 阻塞等 reply。失败时返回 [`Response::Error`],由 trait 实现层兜底。
    fn send_recv(&self, req: Request) -> Response {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.req_tx.send(Pending { req, reply_tx: tx }).is_err() {
            return Response::Error("daemon disconnected".to_owned());
        }
        rx.recv()
            .unwrap_or_else(|_| Response::Error("worker dropped reply".to_owned()))
    }
}

async fn worker(mut conn: Framed<UnixStream>, mut req_rx: mpsc::UnboundedReceiver<Pending>) {
    while let Some(p) = req_rx.recv().await {
        let resp = match round_trip(&mut conn, p.req).await {
            Ok(r) => r,
            Err(e) => {
                mineral_log::warn!(target: "ipc", "round-trip failed: {e}");
                Response::Error(format!("ipc: {e}"))
            }
        };
        let _ = p.reply_tx.send(resp);
    }
}

async fn round_trip(conn: &mut Framed<UnixStream>, req: Request) -> color_eyre::Result<Response> {
    send(conn, &req).await?;
    recv::<Response, _>(conn)
        .await?
        .ok_or_else(|| eyre!("daemon closed connection"))
}

fn warn_unexpected(method: &'static str, resp: &Response) {
    mineral_log::warn!(target: "ipc", "{method}: unexpected response {resp:?}");
}

impl Client for RemoteClient {
    fn play(&self, url: MediaUrl) {
        let _ = self.send_recv(Request::Play(url));
    }
    fn pause(&self) {
        let _ = self.send_recv(Request::Pause);
    }
    fn resume(&self) {
        let _ = self.send_recv(Request::Resume);
    }
    fn stop(&self) {
        let _ = self.send_recv(Request::Stop);
    }
    fn seek(&self, position_ms: u64) {
        let _ = self.send_recv(Request::Seek(position_ms));
    }
    fn set_volume(&self, pct: u8) {
        let _ = self.send_recv(Request::SetVolume(pct));
    }
    fn audio_snapshot(&self) -> AudioSnapshot {
        match self.send_recv(Request::AudioSnapshot) {
            Response::AudioSnapshot(s) => s,
            other => {
                warn_unexpected("audio_snapshot", &other);
                AudioSnapshot::default()
            }
        }
    }
    fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskId {
        match self.send_recv(Request::SubmitTask(kind, priority)) {
            Response::TaskId(id) => id,
            other => {
                warn_unexpected("submit_task", &other);
                // submit 失败时返回 default TaskId(0);调用方都 fire-and-forget,无影响。
                TaskId::default()
            }
        }
    }
    fn cancel_tasks(&self, filter: CancelFilter) {
        let _ = self.send_recv(Request::CancelTasks(filter));
    }
    fn drain_task_events(&self) -> Vec<TaskEvent> {
        match self.send_recv(Request::DrainTaskEvents) {
            Response::TaskEvents(events) => events,
            other => {
                warn_unexpected("drain_task_events", &other);
                Vec::new()
            }
        }
    }
    fn task_snapshot(&self) -> Snapshot {
        match self.send_recv(Request::TaskSnapshot) {
            Response::TaskSnapshot(s) => s,
            other => {
                warn_unexpected("task_snapshot", &other);
                Snapshot {
                    running: 0,
                    by_lane: FxHashMap::default(),
                }
            }
        }
    }
}

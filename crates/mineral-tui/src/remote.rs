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
use mineral_model::{MediaUrl, Song, SongId};
use mineral_protocol::{
    CancelFilter, Framed, PlayerSnapshot, Request, Response, framed, recv, send,
};
use mineral_server::Client;
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};
use rustc_hash::FxHashMap;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

/// 一条等回复的请求。worker 收到后 framed.send → recv → reply_tx.send。
struct Pending {
    /// 待发送的请求。
    req: Request,

    /// 同步 reply 通道发送端;worker 拿到 Response 后 send 一次。
    reply_tx: std::sync::mpsc::Sender<Response>,
}

/// TUI 端的远程 client。`Clone` 通过持 [`mpsc::UnboundedSender`] 廉价。
pub struct RemoteClient {
    /// 把 [`Pending`] 投递给后台 worker;worker drop 时 send 失败,sync 调用方拿到错误兜底。
    req_tx: mpsc::UnboundedSender<Pending>,
}

impl RemoteClient {
    /// 连 daemon socket,起后台 worker。caller 必须在 tokio runtime 里(`mineral-tui::run`
    /// 是 async fn,自然满足)。
    pub async fn connect(socket_path: &Path) -> color_eyre::Result<Self> {
        mineral_log::debug!(target: "ipc", socket_path = %socket_path.display(), "connecting to daemon");
        let stream = UnixStream::connect(socket_path).await.wrap_err_with(|| {
            format!(
                "connect daemon socket {} (run `mineral serve` first?)",
                socket_path.display()
            )
        })?;
        mineral_log::info!(target: "ipc", "connected to daemon");
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

/// 后台 worker:串行收 [`Pending`],打 round-trip,把 Response 回写到 reply 通道。
async fn worker(mut conn: Framed<UnixStream>, mut req_rx: mpsc::UnboundedReceiver<Pending>) {
    while let Some(p) = req_rx.recv().await {
        let resp = match round_trip(&mut conn, p.req).await {
            Ok(r) => r,
            Err(e) => {
                mineral_log::warn!(target: "ipc", error = mineral_log::chain(&e), "round-trip failed");
                Response::Error(format!("ipc: {e}"))
            }
        };
        let _ = p.reply_tx.send(resp);
    }
}

/// 一次 send + recv;daemon 关连接时返回 `Err(...)`,由 worker 翻成 `Response::Error` 兜底。
async fn round_trip(conn: &mut Framed<UnixStream>, req: Request) -> color_eyre::Result<Response> {
    send(conn, &req).await?;
    recv::<Response, _>(conn)
        .await?
        .ok_or_else(|| eyre!("daemon closed connection"))
}

/// 调用方收到非预期 Response 时的统一日志(协议层异常或对端 bug)。
fn warn_unexpected(method: &'static str, resp: &Response) {
    mineral_log::warn!(target: "ipc", method, response = ?resp, "unexpected response");
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
                    by_kind: FxHashMap::default(),
                }
            }
        }
    }

    fn play_song(&self, song: Song) {
        let _ = self.send_recv(Request::PlaySong(Box::new(song)));
    }
    fn set_queue(&self, queue: Vec<Song>, target_id: SongId) {
        let _ = self.send_recv(Request::SetQueue { queue, target_id });
    }
    fn cycle_play_mode(&self) {
        let _ = self.send_recv(Request::CyclePlayMode);
    }
    fn prev_or_restart(&self) {
        let _ = self.send_recv(Request::PrevOrRestart);
    }
    fn next_song(&self) {
        let _ = self.send_recv(Request::NextSong);
    }
    fn player_snapshot(&self) -> PlayerSnapshot {
        match self.send_recv(Request::PlayerSnapshot) {
            Response::PlayerSnapshot(s) => *s,
            other => {
                warn_unexpected("player_snapshot", &other);
                PlayerSnapshot::default()
            }
        }
    }

    fn pull_pcm(&self, n: usize) -> (Vec<f32>, u32) {
        match self.send_recv(Request::PullPcm(n)) {
            Response::PcmData {
                samples,
                sample_rate,
            } => (samples, sample_rate),
            other => {
                warn_unexpected("pull_pcm", &other);
                (Vec::new(), 0)
            }
        }
    }
}

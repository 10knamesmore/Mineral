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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::eyre::{WrapErr, eyre};
use mineral_audio::AudioSnapshot;
use mineral_model::{MediaUrl, Song, SongId};
use mineral_protocol::{
    CancelFilter, DownloadProgress, DownloadTarget, Framed, PlayerSync, PlayerVersions, Request,
    Response, SongStatsWire, framed, recv, send,
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

    /// 链路是否仍可用。worker 检测到 daemon 断开后置 `false`,UI 据此干净退出。
    connected: Arc<AtomicBool>,
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
        let connected = Arc::new(AtomicBool::new(true));
        tokio::spawn(worker(conn, req_rx, Arc::clone(&connected)));
        Ok(Self { req_tx, connected })
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
///
/// round-trip 失败 = 链路断了(unix socket 不重连):打**一条** error 日志、把
/// `connected` 置 `false`,然后退出 worker。worker 退出后 `req_rx` drop,后续
/// [`RemoteClient::send_recv`] 的 `req_tx.send` 直接失败兜底,不再每 tick 刷屏。
async fn worker(
    mut conn: Framed<UnixStream>,
    mut req_rx: mpsc::UnboundedReceiver<Pending>,
    connected: Arc<AtomicBool>,
) {
    while let Some(p) = req_rx.recv().await {
        match round_trip(&mut conn, p.req).await {
            Ok(r) => {
                let _ = p.reply_tx.send(r);
            }
            Err(e) => {
                mineral_log::error!(target: "ipc", error = mineral_log::chain(&e), "daemon connection lost");
                connected.store(false, Ordering::Release);
                let _ = p.reply_tx.send(Response::Error(format!("ipc: {e}")));
                return;
            }
        }
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
    fn player_sync(&self, known: PlayerVersions) -> PlayerSync {
        match self.send_recv(Request::PlayerSync(known)) {
            Response::PlayerSync(s) => *s,
            other => {
                warn_unexpected("player_sync", &other);
                // default 的两个重段是 None(=「与已有一致」),异常时不会把镜像清空。
                PlayerSync::default()
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

    fn toggle_love(&self, id: SongId) -> bool {
        match self.send_recv(Request::ToggleLove(id)) {
            Response::LoveToggled(new) => new,
            // 错误/意外响应:保守返回 false(TUI 乐观值为准)
            _ => false,
        }
    }

    fn query_song_stats(&self, id: SongId) -> Option<SongStatsWire> {
        match self.send_recv(Request::QuerySongStats(id)) {
            Response::SongStats(stats) => stats,
            _ => None,
        }
    }

    fn download(&self, target: DownloadTarget) {
        let _ = self.send_recv(Request::Download(target));
    }

    fn download_progress(&self) -> DownloadProgress {
        match self.send_recv(Request::DownloadProgress) {
            Response::DownloadProgress(p) => p,
            other => {
                warn_unexpected("download_progress", &other);
                DownloadProgress::default()
            }
        }
    }

    fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use mineral_server::Client;
    use tokio::net::UnixListener;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::util::SubscriberInitExt;

    use super::RemoteClient;

    /// 起一个临时 unix socket、accept 一条连接后立刻 drop(模拟 daemon 被 kill /
    /// crash),返回连好的 client(此刻仍 `connected()`)+ socket 路径(测试自行删)。
    ///
    /// 必须 multi-thread runtime:`audio_snapshot` 的 `rx.recv()` 同步阻塞当前线程,
    /// 单线程 runtime 下 worker task 拿不到线程会死锁(生产走多线程 `Runtime::new()`
    /// 不受影响)。
    async fn client_with_dropped_daemon() -> color_eyre::Result<(RemoteClient, PathBuf)> {
        let sock = std::env::temp_dir().join(format!(
            "mineral-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let listener = UnixListener::bind(&sock)?;
        let accept = tokio::spawn(async move {
            if let Ok((stream, _addr)) = listener.accept().await {
                drop(stream);
            }
        });
        let client = RemoteClient::connect(&sock).await?;
        // 确保 server 端已 accept 并 drop(连接确实断了)再返回。
        accept.await?;
        Ok((client, sock))
    }

    /// daemon 断开后:下一次请求触发的 round-trip 失败 → worker 把 `connected` 置
    /// `false`(`send_recv` 阻塞等 reply,worker 在 send reply *之前* store,故返回即翻)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connected_flips_false_when_daemon_drops() -> color_eyre::Result<()> {
        let (client, sock) = client_with_dropped_daemon().await?;
        assert!(client.connected(), "刚连上时应为 connected");

        let _ = client.audio_snapshot();
        assert!(!client.connected(), "daemon 断开后应为 disconnected");

        std::fs::remove_file(&sock)?;
        Ok(())
    }

    /// 进程级日志缓冲;[`capture`] 首次调用时装一个写到这里的全局 subscriber。
    static LOG_BUF: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

    /// 把 tracing 输出收集进共享 buffer 的 [`MakeWriter`]。
    #[derive(Clone)]
    struct BufMaker(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for BufMaker {
        type Writer = BufSink;
        fn make_writer(&'a self) -> Self::Writer {
            BufSink(Arc::clone(&self.0))
        }
    }

    /// [`BufMaker`] 的写端:把字节追加进共享 buffer。
    struct BufSink(Arc<Mutex<Vec<u8>>>);

    impl Write for BufSink {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            let mut guard = self
                .0
                .lock()
                .map_err(|_poison| std::io::Error::other("log buffer poisoned"))?;
            guard.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 装一个把日志写进进程级 buffer 的全局 subscriber(只装一次),返回该 buffer。
    fn capture() -> Arc<Mutex<Vec<u8>>> {
        LOG_BUF
            .get_or_init(|| {
                let buf = Arc::new(Mutex::new(Vec::new()));
                // 测试进程里只此一处装全局 subscriber;已装则 try_init 返回 Err,忽略。
                let _ = tracing_subscriber::fmt()
                    .with_writer(BufMaker(Arc::clone(&buf)))
                    .with_ansi(false)
                    .finish()
                    .try_init();
                buf
            })
            .clone()
    }

    /// daemon 断开时 client 应打一条 error 日志(`daemon connection lost`),不 silent dead。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disconnect_emits_error_log() -> color_eyre::Result<()> {
        let buf = capture();
        let (client, sock) = client_with_dropped_daemon().await?;

        let _ = client.audio_snapshot(); // 触发 round-trip 失败 → worker 打 error 日志
        // worker 在另一线程,给日志写入留一拍。
        tokio::time::sleep(Duration::from_millis(100)).await;

        let logged = {
            let guard = buf
                .lock()
                .map_err(|_poison| color_eyre::eyre::eyre!("log buffer poisoned"))?;
            String::from_utf8_lossy(&guard).into_owned()
        };
        assert!(
            logged.contains("daemon connection lost"),
            "应记录断连 error 日志,实际捕获:\n{logged}"
        );

        std::fs::remove_file(&sock)?;
        Ok(())
    }
}

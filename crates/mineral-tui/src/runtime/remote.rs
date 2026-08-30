//! TUI 端的远程 client:同进程接口,跨进程实现。
//!
//! 实现 [`mineral_server::Client`] trait,所有方法 sync 接口,内部走 unix socket
//! 上的 [`Frame`] 管线。一条长寿命 worker task 持连接:请求经 [`RequestId`] 配对应答,
//! server 可在任意两帧之间交错下推 [`Frame::Event`],
//! event 缓冲在本地,App 每 tick 经 [`Client::drain_events`] 取走。
//! 需当场返回值的 sync 调用通过 std::sync::mpsc 等 reply；后台查询只入队，
//! reply 由 worker 转成 completion event，不阻塞 TUI 调用线程。
//!
//! - **fire-and-forget 类**(play / pause / 等):仍然走 round-trip(协议层 server
//!   每条 Request 必有 Response),但调用方丢弃 Response。
//! - **返回值类**:正常拿 Response 解出来;出错时返回「合理默认值」(`AudioSnapshot::default`
//!   等),错误细节进 `mineral_log::warn`。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use color_eyre::eyre::WrapErr;
use futures_util::{SinkExt, StreamExt};
use mineral_audio::AudioSnapshot;
use mineral_channel_core::ChannelCaps;
use mineral_model::{Song, SongId, SourceKind};
use mineral_protocol::{
    ClientInfo, DownloadProgress, DownloadTarget, Event, Frame, Framed, PlayQueueError, PlayerSync,
    PlayerVersions, QueueContextWire, Request, RequestId, Response, Subscription, client_handshake,
    decode, encode, framed,
};
use mineral_server::Client;
use mineral_task::{Priority, Snapshot, TaskEvent, TaskKind};
use rustc_hash::FxHashMap;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

/// 握手自报的 client 名(TUI 形态;埋点按名归属各 client 的使用时长)。
const CLIENT_NAME: &str = "mineral_tui";

/// 一条待发请求。worker 分配 [`RequestId`] 后发出，应答按 id 交给 completion。
struct Pending {
    /// 待发送的请求。
    req: Request,

    /// 应答的消费方式。
    completion: Completion,
}

/// 配对应答的消费目标。
enum Completion {
    /// 返回给正在等待的 sync 调用方。
    Blocking(std::sync::mpsc::Sender<Response>),

    /// 把本地播放次数投影成 TUI completion event。
    LocalPlayCount {
        /// 查询关联的歌曲。
        song_id: SongId,
    },
}

impl Completion {
    /// 消费一条配对应答。
    ///
    /// # Params:
    ///   - `response`: 与原请求 id 配对的 daemon 应答
    ///   - `events`: completion event 的本地缓冲
    fn resolve(self, response: Response, events: &Mutex<Vec<Event>>) {
        match self {
            Self::Blocking(tx) => drop(tx.send(response)),
            Self::LocalPlayCount { song_id } => {
                let count = match response {
                    Response::SongStats(stats) => stats.map(|stats| stats.play_count),
                    Response::Error(e) => {
                        mineral_log::warn!(
                            target: "ipc",
                            song_id = song_id.as_str(),
                            source = ?song_id.namespace(),
                            message = e,
                            "local play count query failed"
                        );
                        None
                    }
                    other => {
                        warn_unexpected("request_song_stats", &other);
                        None
                    }
                };
                push_event(
                    events,
                    Event::Task(Box::new(TaskEvent::LocalPlayCountFetched {
                        song_id,
                        count,
                    })),
                );
            }
        }
    }
}

/// TUI 端的远程 client。`Clone` 通过持 [`mpsc::UnboundedSender`] 廉价。
pub struct RemoteClient {
    /// 把 [`Pending`] 投递给后台 worker；worker drop 时同步请求拿错误兜底，
    /// completion 请求落空值事件。
    req_tx: mpsc::UnboundedSender<Pending>,

    /// 链路是否仍可用。worker 检测到 daemon 断开后置 `false`,UI 据此干净退出。
    connected: Arc<AtomicBool>,

    /// server 主动推送的 event 缓冲;worker 收到 [`Frame::Event`] 时入列,
    /// App 每 tick 经 [`Client::drain_events`] 取走。
    events: Arc<Mutex<Vec<Event>>>,
}

impl RemoteClient {
    /// 连 daemon socket、完成握手(版本守门 + 订阅集),起后台 worker。
    /// caller 必须在 tokio runtime 里(`mineral-tui::run` 是 async fn,自然满足)。
    ///
    /// # Errors
    /// 连不上 socket / 握手被拒(busy、版本不匹配——错误信息已是人话提示)。
    pub async fn connect(socket_path: &Path) -> color_eyre::Result<Self> {
        mineral_log::debug!(target: "ipc", socket_path = %socket_path.display(), "connecting to daemon");
        let stream = UnixStream::connect(socket_path).await.wrap_err_with(|| {
            format!(
                "connect daemon socket {} (run `mineral serve` first?)",
                socket_path.display()
            )
        })?;
        let mut conn = framed(stream);
        // 握手:订 Toast(提示)+ Lifecycle(ScriptReloaded 刷新 bind 键;
        // 其余生命周期事件收到即忽略)+ Config(有效配置,含握手重放一帧)
        // + WindowTitle(标题覆盖,含握手重放)+ Task(任务 / 数据事件,含
        // 握手重放库快照与收藏集)。Property 暂无消费场景,轮询是权威值来源。
        client_handshake(
            &mut conn,
            ClientInfo::new(
                CLIENT_NAME,
                vec![
                    Subscription::Toast,
                    Subscription::Lifecycle,
                    Subscription::Config,
                    Subscription::WindowTitle,
                    Subscription::Task,
                ],
            ),
        )
        .await?;
        mineral_log::info!(target: "ipc", "connected to daemon");
        let (req_tx, req_rx) = mpsc::unbounded_channel::<Pending>();
        let connected = Arc::new(AtomicBool::new(true));
        let events = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(worker(
            conn,
            req_rx,
            Arc::clone(&connected),
            Arc::clone(&events),
        ));
        Ok(Self {
            req_tx,
            connected,
            events,
        })
    }

    /// 发请求 + 阻塞等配对 reply。失败时返回 [`Response::Error`],由 trait 实现层兜底。
    fn send_recv(&self, req: Request) -> Response {
        let (tx, rx) = std::sync::mpsc::channel();
        if self
            .req_tx
            .send(Pending {
                req,
                completion: Completion::Blocking(tx),
            })
            .is_err()
        {
            return Response::Error("daemon disconnected".to_owned());
        }
        rx.recv()
            .unwrap_or_else(|_| Response::Error("worker dropped reply".to_owned()))
    }
}

/// 后台 worker:持连接的读写两半,`select!` 双路驱动——
/// 请求路:分配 [`RequestId`] 入飞行表后发出;
/// 连接路:`Frame::Response` 按 id 交给 completion、`Frame::Event` 入本地缓冲。
///
/// 断链(读/写失败、EOF、解码失败)= 不重连:打一条 error 日志、置 `connected = false`、
/// 给所有飞行中 completion 回 `Response::Error` 解除阻塞或收束空值,然后退出。worker 退出后 `req_rx`
/// drop,后续 [`RemoteClient::send_recv`] 的 `req_tx.send` 直接失败兜底,不再每 tick 刷屏。
async fn worker(
    conn: Framed<UnixStream>,
    mut req_rx: mpsc::UnboundedReceiver<Pending>,
    connected: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<Event>>>,
) {
    let (mut sink, mut stream) = conn.split();
    let mut next_id: u64 = 0;
    let mut inflight = FxHashMap::<RequestId, Completion>::default();
    loop {
        tokio::select! {
            maybe_pending = req_rx.recv() => {
                let Some(p) = maybe_pending else {
                    return; // RemoteClient 全部释放,worker 随之退出。
                };
                let id = RequestId::new(next_id);
                next_id = next_id.wrapping_add(1);
                let bytes = match encode(&Frame::Request { id, req: p.req }) {
                    Ok(b) => b,
                    Err(e) => {
                        mineral_log::warn!(target: "ipc", error = mineral_log::chain(&e), "请求编码失败");
                        p.completion.resolve(Response::Error(format!("ipc encode: {e}")), &events);
                        continue;
                    }
                };
                inflight.insert(id, p.completion);
                if let Err(e) = sink.send(bytes).await {
                    mineral_log::error!(target: "ipc", error = mineral_log::chain(&e), "daemon connection lost");
                    fail_inflight(&connected, &mut inflight, &events);
                    return;
                }
            }
            maybe_frame = stream.next() => {
                let frame = match maybe_frame {
                    Some(Ok(bytes)) => match decode::<Frame>(&bytes) {
                        Ok(frame) => frame,
                        Err(e) => {
                            mineral_log::error!(target: "ipc", error = mineral_log::chain(&e), "daemon connection lost");
                            fail_inflight(&connected, &mut inflight, &events);
                            return;
                        }
                    },
                    Some(Err(e)) => {
                        mineral_log::error!(target: "ipc", error = mineral_log::chain(&e), "daemon connection lost");
                        fail_inflight(&connected, &mut inflight, &events);
                        return;
                    }
                    None => {
                        mineral_log::error!(target: "ipc", "daemon connection lost");
                        fail_inflight(&connected, &mut inflight, &events);
                        return;
                    }
                };
                match frame {
                    Frame::Response { id, resp } => match inflight.remove(&id) {
                        Some(completion) => completion.resolve(*resp, &events),
                        None => {
                            mineral_log::warn!(target: "ipc", id = id.value(), "无主应答,丢弃");
                        }
                    },
                    Frame::Event(ev) => push_event(&events, ev),
                    other => {
                        mineral_log::warn!(target: "ipc", frame = ?other, "忽略意外帧");
                    }
                }
            }
        }
    }
}

/// 断链收尾:置 disconnected，用 `Response::Error` 收束所有飞行中 completion。
///
/// # Params:
///   - `connected`: client 链路状态
///   - `inflight`: 尚未收到配对应答的 completion
///   - `events`: completion event 的本地缓冲
fn fail_inflight(
    connected: &AtomicBool,
    inflight: &mut FxHashMap<RequestId, Completion>,
    events: &Mutex<Vec<Event>>,
) {
    connected.store(false, Ordering::Release);
    for (_, completion) in inflight.drain() {
        completion.resolve(
            Response::Error("ipc: daemon disconnected".to_owned()),
            events,
        );
    }
}

/// 把一条推送事件压进本地缓冲(中毒锁也能取回数据,不 panic)。
fn push_event(events: &Mutex<Vec<Event>>, ev: Event) {
    events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(ev);
}

/// 调用方收到非预期 Response 时的统一日志(协议层异常或对端 bug)。
fn warn_unexpected(method: &'static str, resp: &Response) {
    mineral_log::warn!(target: "ipc", method, response = ?resp, "unexpected response");
}

impl Client for RemoteClient {
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
    fn submit_task(&self, kind: TaskKind, priority: Priority) {
        match self.send_recv(Request::SubmitTask(kind, priority)) {
            Response::Ok => {}
            other => warn_unexpected("submit_task", &other),
        }
    }
    fn task_snapshot(&self) -> Snapshot {
        match self.send_recv(Request::TaskSnapshot) {
            Response::TaskSnapshot(s) => s,
            other => {
                warn_unexpected("task_snapshot", &other);
                Snapshot {
                    running: 0,
                    by_kind: FxHashMap::default(),
                }
            }
        }
    }

    fn play_song(&self, song: Song) {
        let _ = self.send_recv(Request::PlaySong(Box::new(song)));
    }
    fn play_queue(
        &self,
        songs: Vec<Song>,
        target: usize,
        context: QueueContextWire,
    ) -> Result<(), PlayQueueError> {
        match self.send_recv(Request::PlayQueue {
            songs,
            target,
            context,
        }) {
            Response::PlayQueue(result) => result,
            Response::Error(message) => Err(PlayQueueError::Unavailable { message }),
            other => {
                warn_unexpected("play_queue", &other);
                Err(PlayQueueError::Unavailable {
                    message: "daemon 返回了非 PlayQueue response".to_owned(),
                })
            }
        }
    }
    fn queue_insert_next(&self, song: Song, context: QueueContextWire) {
        let _ = self.send_recv(Request::QueueInsertNext {
            song: Box::new(song),
            context,
        });
    }
    fn queue_append(&self, song: Song, context: QueueContextWire) {
        let _ = self.send_recv(Request::QueueAppend {
            song: Box::new(song),
            context,
        });
    }
    fn queue_edit(&self, op: mineral_protocol::QueueOp) -> mineral_protocol::QueueEditOutcome {
        match self.send_recv(Request::QueueEdit { op }) {
            Response::QueueEdited(outcome) => outcome,
            other => {
                warn_unexpected("queue_edit", &other);
                // 断连 / 应答错位时按「没做成」兜底:报 Applied 会让界面画出一个
                // 服务端并未发生的变更。
                mineral_protocol::QueueEditOutcome::NoOp
            }
        }
    }
    fn channel_caps(&self) -> Vec<(SourceKind, ChannelCaps)> {
        match self.send_recv(Request::ChannelCaps) {
            Response::ChannelCaps(caps) => caps,
            other => {
                warn_unexpected("channel_caps", &other);
                Vec::new()
            }
        }
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

    fn invoke_action(
        &self,
        name: &str,
        ctx: Option<mineral_protocol::KeyContext>,
    ) -> Option<String> {
        match self.send_recv(Request::InvokeAction {
            name: name.to_owned(),
            ctx,
            // 键位触发不带 CLI 位置实参。
            args: Vec::new(),
        }) {
            Response::Error(e) => Some(e),
            _ => None,
        }
    }

    fn render_copy_template(
        &self,
        index: usize,
        ctx: mineral_protocol::CopyTemplateCtx,
    ) -> Result<String, String> {
        match self.send_recv(Request::RenderCopyTemplate { index, ctx }) {
            Response::CopyText(result) => result,
            Response::Error(e) => Err(e),
            other => {
                warn_unexpected("render_copy_template", &other);
                Err("daemon 应答异常(详见日志)".to_owned())
            }
        }
    }

    fn script_binds(&self) -> Vec<mineral_protocol::ScriptBind> {
        match self.send_recv(Request::ScriptBinds) {
            Response::ScriptBinds(binds) => binds,
            // 错误/意外响应:空表兜底(等于无 bind)。
            _ => Vec::new(),
        }
    }

    fn toggle_love(&self, song: Song) -> bool {
        match self.send_recv(Request::ToggleLove(Box::new(song))) {
            Response::LoveToggled(new) => new,
            // 错误/意外响应:保守返回 false(TUI 乐观值为准)
            _ => false,
        }
    }

    fn request_song_stats(&self, id: SongId) {
        if self
            .req_tx
            .send(Pending {
                req: Request::QuerySongStats(id.clone()),
                completion: Completion::LocalPlayCount {
                    song_id: id.clone(),
                },
            })
            .is_err()
        {
            push_event(
                &self.events,
                Event::Task(Box::new(TaskEvent::LocalPlayCountFetched {
                    song_id: id,
                    count: None,
                })),
            );
        }
    }

    fn download(&self, target: DownloadTarget) {
        let _ = self.send_recv(Request::Download(target));
    }

    fn report_terminal_state(&self, rows: u16, cols: u16, fullscreen: bool, focused: bool) {
        let _ = self.send_recv(Request::TerminalState {
            rows,
            cols,
            fullscreen,
            focused,
        });
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

    fn request_daemon_shutdown(&self) {
        // daemon ack 后立即开始收尾;应答可能没写完连接就关,Error 兜底
        // 也视为已投递(`send_recv` 对断连返回 Error,不会卡死)。
        let _ = self.send_recv(Request::Shutdown);
    }

    fn drain_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use color_eyre::eyre::{WrapErr, eyre};
    use mineral_model::{SongId, SourceKind};
    use mineral_protocol::{
        ClientInfo, Event, Frame, PkgVersion, RejectReason, Request, Response, ServerHello,
        SongStatsWire, Subscription, TextSpan, ToastKind, framed, recv, send,
    };
    use mineral_server::Client;
    use tokio::net::{UnixListener, UnixStream};
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::util::SubscriberInitExt;

    use super::RemoteClient;

    /// 把线程 panic 载荷翻译成 Report(捞出 &str / String 信息,不丢上下文)。
    fn join_panic(panic: &(dyn std::any::Any + Send)) -> color_eyre::Report {
        let msg = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "非字符串 panic 载荷".to_owned());
        eyre!("调用方线程 panic: {msg}")
    }

    /// 起一个临时 unix socket,返回 (listener, socket 路径)。路径带 PID+纳秒后缀隔离。
    fn temp_listener() -> color_eyre::Result<(UnixListener, PathBuf)> {
        let sock = std::env::temp_dir().join(format!(
            "mineral-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let listener = UnixListener::bind(&sock)?;
        Ok((listener, sock))
    }

    /// 假 server:accept 一条连接并完成握手(回 accept),把已握手的连接交给 `then`。
    ///
    /// # Params:
    ///   - `listener`: 已 bind 的 listener
    ///   - `then`: 握手完成后的 server 侧剧本
    async fn accept_and_handshake<F, Fut>(listener: UnixListener, then: F) -> color_eyre::Result<()>
    where
        F: FnOnce(mineral_protocol::Framed<UnixStream>) -> Fut + Send,
        Fut: std::future::Future<Output = color_eyre::Result<()>> + Send,
    {
        let (stream, _addr) = listener.accept().await?;
        let mut conn = framed(stream);
        let first: Frame = recv(&mut conn)
            .await?
            .ok_or_else(|| eyre!("server 没收到握手"))?;
        let Frame::Handshake(info) = first else {
            return Err(eyre!("首帧应是 Handshake,实际 {first:?}"));
        };
        assert!(info.version_matches(), "测试两端同 build,版本应一致");
        assert_eq!(
            info.subscriptions,
            vec![
                Subscription::Toast,
                Subscription::Lifecycle,
                Subscription::Config,
                Subscription::WindowTitle,
                Subscription::Task,
            ],
            "TUI 默认订阅集:Toast(提示)+ Lifecycle(ScriptReloaded 刷新 bind 键)+ Config(有效配置)+ WindowTitle(标题覆盖)+ Task(任务 / 数据事件)"
        );
        send(&mut conn, &Frame::Hello(ServerHello::accept())).await?;
        then(conn).await
    }

    /// 起临时 socket + 假 server(握手后立刻断开,模拟 daemon 被 kill / crash),
    /// 返回连好的 client(此刻仍 `connected()`)+ socket 路径(测试自行删)。
    ///
    /// 必须 multi-thread runtime:`audio_snapshot` 的 `rx.recv()` 同步阻塞当前线程,
    /// 单线程 runtime 下 worker task 拿不到线程会死锁(生产走多线程 `Runtime::new()`
    /// 不受影响)。
    async fn client_with_dropped_daemon() -> color_eyre::Result<(RemoteClient, PathBuf)> {
        let (listener, sock) = temp_listener()?;
        let server = tokio::spawn(accept_and_handshake(listener, |conn| async move {
            drop(conn);
            Ok(())
        }));
        let client = RemoteClient::connect(&sock).await?;
        // 确保 server 端握手完成并 drop(连接确实断了)再返回。
        server.await??;
        Ok((client, sock))
    }

    /// server 活着时 connected 为 true(worker 没有误报断连)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connected_true_while_daemon_alive() -> color_eyre::Result<()> {
        let (listener, sock) = temp_listener()?;
        let server = tokio::spawn(accept_and_handshake(listener, |mut conn| async move {
            // 握住连接直到 client 侧 drop(recv 等到 None)。
            let _ = recv::<Frame, _>(&mut conn).await;
            Ok(())
        }));
        let client = RemoteClient::connect(&sock).await?;
        assert!(client.connected(), "server 活着时应为 connected");
        drop(client);
        server.await??;
        std::fs::remove_file(&sock)?;
        Ok(())
    }

    /// daemon 断开后:请求拿到错误兜底,`connected` 翻 `false`(worker 主动发现
    /// EOF,无须等请求触发)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connected_flips_false_when_daemon_drops() -> color_eyre::Result<()> {
        let (client, sock) = client_with_dropped_daemon().await?;

        let _ = client.audio_snapshot();
        assert!(!client.connected(), "daemon 断开后应为 disconnected");

        std::fs::remove_file(&sock)?;
        Ok(())
    }

    /// 握手被拒(版本不匹配)时 connect 直接报人话错误,不起 worker。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connect_rejected_handshake_bails() -> color_eyre::Result<()> {
        let (listener, sock) = temp_listener()?;
        let server = tokio::spawn(async move {
            let (stream, _addr) = listener.accept().await?;
            let mut conn = framed(stream);
            // 拒绝不等握手帧,直接回 Hello。
            send(
                &mut conn,
                &Frame::Hello(ServerHello::reject(RejectReason::VersionMismatch)),
            )
            .await?;
            // 握住连接直到 client 收到拒绝主动断开(立刻 drop 会让 client 的握手帧
            // 撞上已关 socket,错误变成 EPIPE 而非拒绝原因,测试 flaky)。
            while recv::<Frame, _>(&mut conn).await.is_ok_and(|f| f.is_some()) {}
            Ok::<(), color_eyre::Report>(())
        });

        let err = match RemoteClient::connect(&sock).await {
            Ok(_) => return Err(eyre!("被拒的握手不该成功")),
            Err(e) => format!("{e:#}"),
        };
        assert!(err.contains("版本"), "错误信息应说明版本不匹配,实际:{err}");
        server.await??;
        std::fs::remove_file(&sock)?;
        Ok(())
    }

    /// 乱序应答按 id 配对:两条并发请求,server 倒序回包 + 中间夹一条 Event——
    /// 每个调用方仍拿到自己的应答,Event 进缓冲。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn out_of_order_replies_pair_by_id() -> color_eyre::Result<()> {
        let (listener, sock) = temp_listener()?;
        let server = tokio::spawn(accept_and_handshake(listener, |mut conn| async move {
            // 收两条请求(到达顺序即 id 顺序)。
            let mut got = Vec::new();
            for _ in 0..2_u8 {
                let frame: Frame = recv(&mut conn)
                    .await?
                    .ok_or_else(|| eyre!("server 没收到请求"))?;
                let Frame::Request { id, req } = frame else {
                    return Err(eyre!("应是 Request"));
                };
                got.push((id, req));
            }
            // 中间夹一条 Event,再**倒序**回两条应答。
            send(
                &mut conn,
                &Frame::Event(Event::Toast {
                    kind: ToastKind::Info,
                    content: vec![TextSpan::plain("插队事件")],
                    id: None,
                    ttl_secs: None,
                }),
            )
            .await?;
            for (id, req) in got.into_iter().rev() {
                let resp = match req {
                    Request::AudioSnapshot => {
                        Response::AudioSnapshot(mineral_audio::AudioSnapshot::default())
                    }
                    Request::DaemonInfo => Response::DaemonInfo { pid: 4242 },
                    other => return Err(eyre!("意外请求 {other:?}")),
                };
                send(
                    &mut conn,
                    &Frame::Response {
                        id,
                        resp: Box::new(resp),
                    },
                )
                .await?;
            }
            // 等 client 侧断开再退出,避免连接早关导致解码错误日志。
            let _ = recv::<Frame, _>(&mut conn).await;
            Ok(())
        }));

        let client = Arc::new(RemoteClient::connect(&sock).await?);
        // 两个阻塞调用方并发在飞:audio_snapshot(id=0)与 DaemonInfo(id=1)。
        let a = {
            let client = Arc::clone(&client);
            std::thread::spawn(move || client.audio_snapshot())
        };
        let b = {
            let client = Arc::clone(&client);
            std::thread::spawn(move || client.send_recv(Request::DaemonInfo))
        };
        let snap = a.join().map_err(|p| join_panic(p.as_ref()))?;
        assert_eq!(
            snap,
            mineral_audio::AudioSnapshot::default(),
            "乱序回包后 audio_snapshot 仍应配对到快照应答"
        );
        let info = b.join().map_err(|p| join_panic(p.as_ref()))?;
        let Response::DaemonInfo { pid } = info else {
            return Err(eyre!("DaemonInfo 应配对到 DaemonInfo 应答,实际 {info:?}"));
        };
        assert_eq!(pid, 4242);

        // 交错的 Event 应已进缓冲(应答晚于 Event 上线,顺序有保证)。
        let events = client.drain_events();
        assert_eq!(events.len(), 1, "应缓冲恰好一条 Event");
        assert!(
            matches!(
                events.first(),
                Some(Event::Toast { content, .. }) if *content == vec![TextSpan::plain("插队事件")]
            ),
            "缓冲的应是插队 Toast,实际 {events:?}"
        );
        assert!(client.drain_events().is_empty(), "drain 是消费式语义");

        drop(client); // worker 退出 → server 侧 recv 返回 None。
        server.await??;
        std::fs::remove_file(&sock)?;
        Ok(())
    }

    /// Song stats 查询只入队，不在 TUI 调用线程等 daemon 的 DB 结果。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn song_stats_request_returns_before_delayed_response() -> color_eyre::Result<()> {
        let (listener, sock) = temp_listener()?;
        let server = tokio::spawn(accept_and_handshake(listener, |mut conn| async move {
            let frame: Frame = recv(&mut conn)
                .await?
                .ok_or_else(|| eyre!("server 没收到统计请求"))?;
            let Frame::Request { id, req } = frame else {
                return Err(eyre!("应是 Request"));
            };
            assert!(
                matches!(req, Request::QuerySongStats(_)),
                "应发 QuerySongStats，实际 {req:?}"
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
            send(
                &mut conn,
                &Frame::Response {
                    id,
                    resp: Box::new(Response::SongStats(Some(SongStatsWire {
                        play_count: 7,
                        skip_count: 2,
                        total_listen_ms: 123_000,
                        last_played_at: Some(456),
                        loved: false,
                    }))),
                },
            )
            .await?;
            let _ = recv::<Frame, _>(&mut conn).await;
            Ok(())
        }));

        let client = RemoteClient::connect(&sock).await?;
        let id = SongId::new(SourceKind::BILIBILI, "b1");
        let started = std::time::Instant::now();
        client.request_song_stats(id.clone());
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "request_song_stats 不应等待 300ms response"
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut fetched = None;
        while std::time::Instant::now() < deadline {
            for event in client.drain_events() {
                if let Event::Task(task) = event
                    && let mineral_task::TaskEvent::LocalPlayCountFetched { song_id, count } = *task
                {
                    fetched = Some((song_id, count));
                }
            }
            if fetched.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(fetched, Some((id, Some(7))));

        drop(client);
        server.await??;
        std::fs::remove_file(&sock)?;
        Ok(())
    }

    /// 起进程内真 Server(ForceNull 音频 + 禁用 persist)并跑 serve loop。
    ///
    /// # Return:
    ///   (event hub 发送端, serve loop task, socket 路径)。caller 负责 abort task、删 socket。
    async fn spawn_inproc_server() -> color_eyre::Result<(
        tokio::sync::broadcast::Sender<Event>,
        tokio::task::JoinHandle<color_eyre::Result<()>>,
        PathBuf,
    )> {
        let cfg = mineral_config::Config::defaults()?;
        let server = mineral_server::Server::spawn(
            mineral_server::SourceBackends::builder()
                .channels(Vec::new())
                .playback(mineral_playback::PlaybackRegistry::empty())
                .build(),
            mineral_server::AudioMode::ForceNull,
            mineral_persist::ServerStore::disabled(),
            mineral_server::ServerConfig::from_config(&cfg),
            mineral_config::default_tree()?,
            /*script*/ None,
            mineral_server::StatsRecorder::disabled(),
        )
        .await?;
        let (listener, sock) = temp_listener()?;
        let sink = server.event_sink();
        let serve = tokio::spawn(async move { server.serve(listener).await });
        Ok((sink, serve, sock))
    }

    /// 进程内全链路:Server 的 broadcast hub → serve loop(先订阅再回 Hello +
    /// 订阅集过滤 + Frame 下推)→ RemoteClient 缓冲 → `drain_events`。
    /// 同时验证:握手先重放一帧有效配置(订 Config 的语义),未订阅的
    /// PropertyChanged 被过滤不到达。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn event_push_end_to_end() -> color_eyre::Result<()> {
        let (sink, serve, sock) = spawn_inproc_server().await?;

        let client = RemoteClient::connect(&sock).await?;
        // 未订阅类别先推(若过滤失效,它会先于 Toast 到达而被断言抓到)。
        let _ = sink.send(Event::PropertyChanged {
            prop: mineral_protocol::PropName::PLAYER_VOLUME,
            value: mineral_protocol::PropValue::Int(42),
        });
        let _ = sink.send(Event::Toast {
            kind: ToastKind::Info,
            content: vec![TextSpan::plain("hub 直推")],
            id: None,
            ttl_secs: None,
        });

        // 轮询累积到 Toast 到达为止(有界等待,防 flaky);重放帧先于它入队。
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut events = Vec::<Event>::new();
        loop {
            events.extend(client.drain_events());
            if events.iter().any(|e| matches!(e, Event::Toast { .. })) {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(eyre!("2s 内没收到下推 Toast:{events:?}"));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            events.len(),
            2,
            "应恰好两条:握手重放的 ConfigChanged + hub 推的 Toast:{events:?}"
        );
        assert!(
            matches!(events.first(), Some(Event::ConfigChanged { .. })),
            "首帧应是握手重放的有效配置,实际 {events:?}"
        );
        assert!(
            matches!(
                events.get(1),
                Some(Event::Toast { content, .. }) if *content == vec![TextSpan::plain("hub 直推")]
            ),
            "第二条应是 hub 推的 Toast,实际 {events:?}"
        );

        serve.abort();
        drop(client);
        std::fs::remove_file(&sock)?;
        Ok(())
    }

    /// 真 server 的多 client 接入:两个连接并存都被服务,先到者不被顶掉;
    /// 一个断开不影响另一个。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_clients_both_served() -> color_eyre::Result<()> {
        let (_sink, serve, sock) = spawn_inproc_server().await?;

        let first = RemoteClient::connect(&sock).await?;
        let second = RemoteClient::connect(&sock)
            .await
            .wrap_err("第二个连接应被接受而非拒绝")?;
        // 两连接同时可服务(轮询请求各自成功)。
        let _ = first.audio_snapshot();
        let _ = second.audio_snapshot();
        assert!(first.connected(), "先到连接不被后来者顶掉");
        assert!(second.connected(), "后到连接照常服务");

        drop(first);
        // 断开是异步收尾;另一连接不受影响,请求持续成功。
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = second.audio_snapshot();
        assert!(second.connected(), "一个断开不影响另一个");
        serve.abort();
        std::fs::remove_file(&sock)?;
        Ok(())
    }

    /// 真 server 的版本守门:报过期版本的握手被拒,reason 是 VersionMismatch
    /// 且 Hello 带 server 真实版本。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_version_rejected_by_server() -> color_eyre::Result<()> {
        let (_sink, serve, sock) = spawn_inproc_server().await?;

        let stream = UnixStream::connect(&sock).await?;
        let mut conn = framed(stream);
        let stale = ClientInfo {
            name: super::CLIENT_NAME.to_owned(),
            version: PkgVersion {
                major: 0,
                minor: 0,
                patch: 0,
            },
            subscriptions: Vec::new(),
        };
        send(&mut conn, &Frame::Handshake(stale)).await?;
        let hello = match recv::<Frame, _>(&mut conn).await? {
            Some(Frame::Hello(hello)) => hello,
            other => return Err(eyre!("应收到 Hello,实际 {other:?}")),
        };
        assert!(!hello.accepted, "过期版本不该被接受");
        assert_eq!(hello.reason, Some(RejectReason::VersionMismatch));
        assert_eq!(
            hello.version,
            PkgVersion::current(),
            "Hello 应带 server 真实版本"
        );

        serve.abort();
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

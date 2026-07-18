//! IPC accept loop + 单 connection 的 [`Frame`] 管线:握手守门 → 读循环并发
//! dispatch → **唯一 writer** 串行下发(Response 与订阅过滤后的 Event 汇同一条
//! mpsc,杜绝并发写 sink)。
//!
//! 多 client:每条 connection 独立 task / writer / event 订阅,连接间无共享
//! 可变通道;在线身份经 [`ConnRegistry`] 登记,断开(含 panic unwind)即移除。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use color_eyre::eyre::WrapErr;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use mineral_protocol::{
    ClientInfo, Event, Frame, Framed, RejectReason, Request, Response, ServerHello, Subscription,
    decode, encode, framed, recv, send,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast, mpsc};

use crate::client::{Client, ClientHandle};

/// Accept loop。返回 `Ok(())` 仅在 listener 被外部关闭时;否则一直循环。
///
/// `on_connect` 在每条新 connection 被接受后立刻调用一次,调用方借此重新触发
/// 「初始数据加载」(`MyPlaylists` 任务 + 收藏同步)——必要的:`drain_task_events`
/// 与 client_events 都是消费式语义,首个 client 拿走 events 后 buffer 清空,新 client
/// 看不到任何历史 event 会显示「数据为空」假象。dedup 命中既存任务时无副作用。
///
/// `shutdown` 是 daemon 级关停通知:client 发 [`Request::Shutdown`] 时在此
/// 唤醒,由 daemon 入口的 select 接走、走与 SIGTERM 相同的 graceful 收尾。
pub(crate) async fn run<F>(
    listener: UnixListener,
    client: ClientHandle,
    registry: Arc<ConnRegistry>,
    events: broadcast::Sender<Event>,
    on_connect: F,
    shutdown: Arc<Notify>,
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
        let conn_id = registry.register();
        on_connect();
        mineral_log::info!(target: "ipc", conn_id, "client connected");
        // per-connection handle:后续所有 per-conn 状态(终端上报 / PCM 游标)
        // 都以这个 id 归属,断开随 guard 一体清理。
        let client = client.for_connection(conn_id);
        let guard = ConnGuard {
            registry: Arc::clone(&registry),
            id: conn_id,
            client: client.clone(),
        };
        let registry = Arc::clone(&registry);
        let events = events.clone();
        let shutdown = Arc::clone(&shutdown);
        tokio::spawn(async move {
            // 守卫持有到 task 结束:正常返回与 panic unwind 都从注册表移除,
            // 杜绝一次连接事故留下幽灵在线记录。
            let _guard = guard;
            if let Err(e) =
                handle_connection(stream, &client, &registry, conn_id, &events, &shutdown).await
            {
                mineral_log::warn!(target: "ipc", conn_id, error = mineral_log::chain(&e), "connection ended with error");
            }
        });
    }
}

/// 已接入连接的注册表:accept 分配自增 id、断开移除;心跳读在线数,握手后
/// 登记 client 身份(版本 + 订阅集)。
pub(crate) struct ConnRegistry {
    /// 下一个连接 id(进程内单调自增,不复用)。
    next_id: AtomicU64,

    /// 在线连接:id → 连接元数据。
    conns: parking_lot::Mutex<rustc_hash::FxHashMap<u64, ConnMeta>>,
}

/// 一条在线连接的元数据(断开时结算 client_connections 埋点用)。
struct ConnMeta {
    /// 握手身份(握手完成前为 `None`;埋点只结算握手完成的连接)。
    identity: Option<ClientInfo>,

    /// 连接建立时刻。
    connected_at: std::time::Instant,

    /// 建立时刻的在线连接数(含自己)。
    concurrent_at_connect: usize,
}

impl ConnRegistry {
    /// 空注册表。
    pub(crate) fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            conns: parking_lot::Mutex::new(rustc_hash::FxHashMap::default()),
        }
    }

    /// 登记一条新连接,返回其 id。
    fn register(&self) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut conns = self.conns.lock();
        let concurrent_at_connect = conns.len() + 1;
        conns.insert(
            id,
            ConnMeta {
                identity: None,
                connected_at: std::time::Instant::now(),
                concurrent_at_connect,
            },
        );
        id
    }

    /// 握手完成,补登身份。
    fn set_identity(&self, id: u64, info: ClientInfo) {
        if let Some(meta) = self.conns.lock().get_mut(&id) {
            meta.identity = Some(info);
        }
    }

    /// 连接断开,移除并交回元数据(埋点结算用)。
    fn unregister(&self, id: u64) -> Option<ConnMeta> {
        self.conns.lock().remove(&id)
    }

    /// 当前在线连接数(心跳上报用)。
    pub(crate) fn online(&self) -> usize {
        self.conns.lock().len()
    }
}

/// 注册表移除守卫:连接 task 无论正常结束还是 panic unwind,drop 时都移除,
/// 并结算 client_connections 埋点(只记握手完成的连接;daemon 停机时进程直接
/// 退出、guard 不保证跑到,在线连接丢行——与硬 kill 丢在播行同类取舍)。
struct ConnGuard {
    /// 所属注册表。
    registry: Arc<ConnRegistry>,

    /// 本连接 id。
    id: u64,

    /// 埋点出口(per-conn handle)。
    client: ClientHandle,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        let Some(meta) = self.registry.unregister(self.id) else {
            return;
        };
        let Some(identity) = meta.identity else {
            return; // 未握手即走(探活 / 被拒),不落行。
        };
        let duration_ms =
            i64::try_from(meta.connected_at.elapsed().as_millis()).unwrap_or(i64::MAX);
        let concurrent = i64::try_from(meta.concurrent_at_connect).unwrap_or(i64::MAX);
        self.client
            .record_client_connection(identity.name, duration_ms, concurrent);
    }
}

/// 接管已 accept 的 connection:握手守门 → split 出读写两半 → 起唯一 writer 与
/// 推送泵 → 读循环到 client EOF / 出错,随后收尾(泵停、writer 排空退出)。
async fn handle_connection(
    stream: UnixStream,
    client: &ClientHandle,
    registry: &ConnRegistry,
    conn_id: u64,
    events: &broadcast::Sender<Event>,
    shutdown: &Arc<Notify>,
) -> color_eyre::Result<()> {
    let mut conn = framed(stream);
    // 在回 Hello 之前就订阅 hub:保证「client 收到 Hello」之后产生的事件零窗口
    // 不丢(握手期间的事件缓冲在 receiver 里,由 pump 按订阅集过滤)。
    let events_rx = events.subscribe();
    let Some(info) = handshake(&mut conn, client).await? else {
        return Ok(());
    };
    let subscriptions = info.subscriptions.clone();
    registry.set_identity(conn_id, info);
    let (sink, stream) = conn.split();
    // Response 与 Event 汇同一条 mpsc → 唯一 writer 串行写 sink,杜绝并发写。
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Frame>();
    tokio::spawn(write_loop(sink, out_rx));
    // 订阅类别的当前状态先重放,再进实时流(pump 还没起,重放帧必然先入队;
    // events_rx 在握手前已订阅,期间的变更不丢——快照与缓冲间可能重复一帧,
    // client 侧 last-wins 幂等)。重放内容按类别见 [`ClientHandle::replay_frames`]。
    for ev in client.replay_frames(&subscriptions).await {
        let _ = out_tx.send(Frame::Event(ev));
    }
    let pump = tokio::spawn(event_pump(events_rx, subscriptions, out_tx.clone()));
    let result = read_loop(stream, client, &out_tx, shutdown).await;
    // client 断开:清本连接的终端上报与 PCM 游标(全部离线时 `terminal`
    // 属性回 None,脚本可感知离线)。
    client.connection_closed();
    // 收尾:推送泵立停、放掉本端 out_tx。writer **不 await**——飞行中的 dispatch
    // task(如 love 的远端打点,可达数秒)还持着 out_tx clone,等它们结束才轮到
    // writer 退出;把 busy 的释放拖到慢 dispatch 之后,会让紧接着重连的 client
    // 被误拒 busy。writer 自带退出条件(全部 sender 放掉 / 写失败),晚到的应答
    // 尽力而为地写(对端已走则写失败、writer 自行退出),不泄漏。
    pump.abort();
    drop(out_tx);
    result
}

/// 握手守门:期待首帧 [`Frame::Handshake`],版本不匹配回拒绝;通过回 accept。
///
/// # Return:
///   `Some(client 身份)` = 握手通过;`None` = 连接该就此关闭(拒绝 / 对端探活
///   即走 / 首帧不是握手)。
async fn handshake(
    conn: &mut Framed<UnixStream>,
    client: &ClientHandle,
) -> color_eyre::Result<Option<ClientInfo>> {
    let info = match recv::<Frame, _>(conn).await.wrap_err("等待握手帧")? {
        Some(Frame::Handshake(info)) => info,
        Some(other) => {
            mineral_log::warn!(target: "ipc", frame = ?other, "首帧不是握手,断开");
            return Ok(None);
        }
        // 连上没说话就走(探活类),不算错。
        None => return Ok(None),
    };
    if !info.version_matches() {
        mineral_log::warn!(
            target: "ipc",
            client_version = %info.version,
            "client 版本不匹配,拒绝连接"
        );
        client.record_connection_reject(mineral_stats::RejectReason::VersionMismatch);
        send(
            conn,
            &Frame::Hello(ServerHello::reject(RejectReason::VersionMismatch)),
        )
        .await?;
        return Ok(None);
    }
    send(conn, &Frame::Hello(ServerHello::accept())).await?;
    mineral_log::debug!(target: "ipc", subscriptions = ?info.subscriptions, "handshake accepted");
    Ok(Some(info))
}

/// 唯一 writer:把汇聚的 [`Frame`] 串行写进 sink。写失败即退出(连接已断,
/// 读循环也会随之退出);编码失败跳过该帧(单帧损坏不拖垮连接)。
async fn write_loop(
    mut sink: SplitSink<Framed<UnixStream>, bytes::Bytes>,
    mut rx: mpsc::UnboundedReceiver<Frame>,
) {
    while let Some(frame) = rx.recv().await {
        let bytes = match encode(&frame) {
            Ok(b) => b,
            Err(e) => {
                mineral_log::warn!(target: "ipc", error = mineral_log::chain(&e), "frame 编码失败,跳过");
                continue;
            }
        };
        if let Err(e) = sink.send(bytes).await {
            mineral_log::warn!(target: "ipc", error = mineral_log::chain(&e), "写连接失败,writer 退出");
            return;
        }
    }
}

/// 推送泵:订阅 hub,按握手订阅集过滤,转成 [`Frame::Event`] 汇入 writer。
/// Lagged 仅 warn 丢弃(event 是 advisory,丢了下个 tick 轮询兜底);
/// hub 关闭或连接收尾即退出。
async fn event_pump(
    mut rx: broadcast::Receiver<Event>,
    subscriptions: Vec<Subscription>,
    out: mpsc::UnboundedSender<Frame>,
) {
    loop {
        match rx.recv().await {
            Ok(ev) => {
                if !subscriptions.contains(&ev.subscription()) {
                    continue;
                }
                if out.send(Frame::Event(ev)).is_err() {
                    return; // writer 已收尾。
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                mineral_log::warn!(target: "ipc", skipped, "event 下推积压,丢弃滞后事件");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// 读循环:每条 [`Frame::Request`] spawn 并发 dispatch,应答带原 id 汇入 writer;
/// 其余帧(重复握手等)warn 后忽略。client EOF 返回 `Ok`。
async fn read_loop(
    mut stream: SplitStream<Framed<UnixStream>>,
    client: &ClientHandle,
    out: &mpsc::UnboundedSender<Frame>,
    shutdown: &Arc<Notify>,
) -> color_eyre::Result<()> {
    while let Some(frame) = stream.next().await {
        let bytes = frame.wrap_err("framed recv")?;
        match decode::<Frame>(&bytes)? {
            Frame::Request { id, req } => {
                let client = client.clone();
                let out = out.clone();
                let shutdown = Arc::clone(shutdown);
                let is_shutdown = matches!(req, Request::Shutdown);
                tokio::spawn(async move {
                    let resp = dispatch(req, &client).await;
                    // send 失败 = 连接已收尾,应答丢弃即可。
                    let _ = out.send(Frame::Response {
                        id,
                        resp: Box::new(resp),
                    });
                    // ack 先入队、再唤醒关停:给 writer 把应答写出去的机会。
                    // 尽力而为——client 侧不依赖这条 ack(连接被关、读到 EOF
                    // 同样视为 daemon 正在退)。
                    if is_shutdown {
                        shutdown.notify_one();
                    }
                });
            }
            other => {
                mineral_log::warn!(target: "ipc", frame = ?other, "忽略非 Request 帧");
            }
        }
    }
    Ok(())
}

/// [`Request`] 到 [`Response`] 的 dispatch:每条 variant 对应一个 [`Client`]
/// trait 方法或 [`ClientHandle`] 固有方法调用,组装成预期的 Response variant。
///
/// sync trait 方法在出错时返回值类用「合理默认值」兜底,不产生 [`Response::Error`];
/// love / 统计这类有真实失败语义的 async 方法出错时收敛成 [`Response::Error`]
/// (经 [`mineral_log::chain`] 展开 context 链)。
async fn dispatch(req: Request, client: &ClientHandle) -> Response {
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
        Request::TaskSnapshot => Response::TaskSnapshot(client.task_snapshot()),
        Request::PlaySong(song) => {
            client.play_song(*song);
            Response::Ok
        }
        Request::SetQueue {
            queue,
            target_id,
            context,
        } => {
            client.set_queue(queue, target_id, context);
            Response::Ok
        }
        Request::QueueInsertNext { song, context } => {
            client.queue_insert_next(*song, context);
            Response::Ok
        }
        Request::QueueAppend { song, context } => {
            client.queue_append(*song, context);
            Response::Ok
        }
        Request::ChannelCaps => Response::ChannelCaps(client.channel_caps()),
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
        Request::PlayerSync(known) => Response::PlayerSync(Box::new(client.player_sync(known))),
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
        Request::InvokeAction { name, ctx, args } => {
            match client.invoke_action_async(&name, ctx, args).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(mineral_log::chain(&e)),
            }
        }
        Request::RenderCopyTemplate { index, ctx } => {
            Response::CopyText(client.render_copy_template_async(index, ctx).await)
        }
        Request::StoreGet { song, key } => match client.store_get_async(&song, &key).await {
            Ok(value) => Response::StoreValue(value),
            Err(e) => Response::Error(mineral_log::chain(&e)),
        },
        Request::StoreSet { song, key, value } => {
            match client.store_set_async(&song, &key, &value).await {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(mineral_log::chain(&e)),
            }
        }
        Request::StoreInc { song, key, delta } => {
            match client.store_inc_async(&song, &key, delta).await {
                Ok(value) => Response::StoreValue(value),
                Err(e) => Response::Error(mineral_log::chain(&e)),
            }
        }
        Request::ScriptBinds => Response::ScriptBinds(client.script_binds_async().await),
        Request::ToggleLove(song) => match client.toggle_love_async(&song).await {
            Ok(new) => Response::LoveToggled(new),
            Err(e) => Response::Error(mineral_log::chain(&e)),
        },
        Request::QuerySongStats(id) => match client.query_song_stats_async(&id).await {
            Ok(stats) => Response::SongStats(stats),
            Err(e) => Response::Error(mineral_log::chain(&e)),
        },
        Request::Download(target) => {
            client.download(target);
            Response::Ok
        }
        Request::DownloadProgress => Response::DownloadProgress(client.download_progress()),
        Request::TerminalState {
            rows,
            cols,
            fullscreen,
            focused,
        } => {
            client.report_terminal_state(rows, cols, fullscreen, focused);
            Response::Ok
        }
        // 实际唤醒在 read_loop 的 dispatch task 里(ack 入队之后),这里只
        // ack + 留痕;e2e 据这行日志断言关停原因。
        Request::Shutdown => {
            mineral_log::info!(target: "ipc", "shutdown requested via IPC");
            Response::Ok
        }
    }
}

/// 请求的日志名;高频轮询类(snapshot / pcm / drain,TUI 每 tick 拉)返回 `None`
/// 不记,避免刷屏。其余状态变更类返回 variant 名。
fn req_log_name(req: &Request) -> Option<&'static str> {
    match req {
        Request::AudioSnapshot
        | Request::PlayerSync(_)
        | Request::TaskSnapshot
        | Request::DownloadProgress
        // 拖动 resize 会连发,不记。
        | Request::TerminalState { .. }
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
        Request::QueueInsertNext { .. } => Some("QueueInsertNext"),
        Request::QueueAppend { .. } => Some("QueueAppend"),
        Request::ChannelCaps => Some("ChannelCaps"),
        Request::CyclePlayMode => Some("CyclePlayMode"),
        Request::PrevOrRestart => Some("PrevOrRestart"),
        Request::NextSong => Some("NextSong"),
        Request::DaemonInfo => Some("DaemonInfo"),
        Request::InvokeAction { .. } => Some("InvokeAction"),
        Request::RenderCopyTemplate { .. } => Some("RenderCopyTemplate"),
        Request::StoreGet { .. } => Some("StoreGet"),
        Request::StoreSet { .. } => Some("StoreSet"),
        Request::StoreInc { .. } => Some("StoreInc"),
        Request::ScriptBinds => Some("ScriptBinds"),
        Request::ToggleLove(_) => Some("ToggleLove"),
        Request::QuerySongStats(_) => Some("QuerySongStats"),
        Request::Download(_) => Some("Download"),
        Request::Shutdown => Some("Shutdown"),
    }
}

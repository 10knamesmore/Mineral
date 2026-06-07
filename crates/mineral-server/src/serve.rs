//! IPC accept loop + 单 connection 的 [`Frame`] 管线:握手守门 → 读循环并发
//! dispatch → **唯一 writer** 串行下发(Response 与订阅过滤后的 Event 汇同一条
//! mpsc,杜绝并发写 sink)。
//!
//! 单 client 限制保留:已有 connection 时,新 incoming **不等握手**直接收
//! `Hello { accepted: false, reason: Busy }` 然后被关。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::eyre::WrapErr;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use mineral_protocol::{
    Event, Frame, Framed, RejectReason, Request, Response, ServerHello, Subscription, decode,
    encode, framed, recv, send,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

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
    events: broadcast::Sender<Event>,
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
            // 已经有 client 在用 → 不等握手直接拒。失败也无所谓,client 自己会 EOF。
            mineral_log::warn!(target: "ipc", "rejected new connection: single-client busy");
            tokio::spawn(reject_busy(stream));
            continue;
        }
        on_connect();
        mineral_log::info!(target: "ipc", "client connected");
        let client = client.clone();
        let busy_guard = BusyGuard(Arc::clone(&busy));
        let events = events.clone();
        tokio::spawn(async move {
            // 守卫持有到 task 结束:正常返回与 panic unwind 都复位 busy,
            // 杜绝 daemon 因一次连接事故永久拒连。
            let _busy_guard = busy_guard;
            if let Err(e) = handle_connection(stream, &client, &events).await {
                mineral_log::warn!(target: "ipc", error = mineral_log::chain(&e), "connection ended with error");
            }
        });
    }
}

/// 单 client 占用标志的复位守卫:连接 task 无论正常结束还是 panic unwind,
/// drop 时都复位 busy。
struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// 已有 client 在用:新连进来的不等握手,直接收 `Hello { Busy }` 后被关。
async fn reject_busy(stream: UnixStream) {
    let mut conn = framed(stream);
    let _ = send(
        &mut conn,
        &Frame::Hello(ServerHello::reject(RejectReason::Busy)),
    )
    .await;
}

/// 接管已 accept 的 connection:握手守门 → split 出读写两半 → 起唯一 writer 与
/// 推送泵 → 读循环到 client EOF / 出错,随后收尾(泵停、writer 排空退出)。
async fn handle_connection(
    stream: UnixStream,
    client: &ClientHandle,
    events: &broadcast::Sender<Event>,
) -> color_eyre::Result<()> {
    let mut conn = framed(stream);
    // 在回 Hello 之前就订阅 hub:保证「client 收到 Hello」之后产生的事件零窗口
    // 不丢(握手期间的事件缓冲在 receiver 里,由 pump 按订阅集过滤)。
    let events_rx = events.subscribe();
    let Some(subscriptions) = handshake(&mut conn).await? else {
        return Ok(());
    };
    let (sink, stream) = conn.split();
    // Response 与 Event 汇同一条 mpsc → 唯一 writer 串行写 sink,杜绝并发写。
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Frame>();
    tokio::spawn(write_loop(sink, out_rx));
    // 订阅 UiOverride 的 client 先收覆盖表重放,再进实时流(pump 还没起,
    // 重放帧必然先入队;events_rx 在握手前已订阅,期间的变更不丢——
    // 快照与缓冲间可能重复一条,client 侧 last-wins 幂等)。
    if subscriptions.contains(&Subscription::UiOverride) {
        for (key, value) in client.ui_overrides_snapshot() {
            let _ = out_tx.send(Frame::Event(Event::UiOverride {
                key,
                value: Some(value),
            }));
        }
    }
    let pump = tokio::spawn(event_pump(events_rx, subscriptions, out_tx.clone()));
    let result = read_loop(stream, client, &out_tx).await;
    // client 断开:清终端上报,`terminal` 属性回 None(脚本可感知离线)。
    client.clear_terminal_state();
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
///   `Some(订阅集)` = 握手通过;`None` = 连接该就此关闭(拒绝 / 对端探活即走 /
///   首帧不是握手)。
async fn handshake(conn: &mut Framed<UnixStream>) -> color_eyre::Result<Option<Vec<Subscription>>> {
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
        send(
            conn,
            &Frame::Hello(ServerHello::reject(RejectReason::VersionMismatch)),
        )
        .await?;
        return Ok(None);
    }
    send(conn, &Frame::Hello(ServerHello::accept())).await?;
    mineral_log::debug!(target: "ipc", subscriptions = ?info.subscriptions, "handshake accepted");
    Ok(Some(info.subscriptions))
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
) -> color_eyre::Result<()> {
    while let Some(frame) = stream.next().await {
        let bytes = frame.wrap_err("framed recv")?;
        match decode::<Frame>(&bytes)? {
            Frame::Request { id, req } => {
                let client = client.clone();
                let out = out.clone();
                tokio::spawn(async move {
                    let resp = dispatch(req, &client).await;
                    // send 失败 = 连接已收尾,应答丢弃即可。
                    let _ = out.send(Frame::Response {
                        id,
                        resp: Box::new(resp),
                    });
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
        Request::InvokeAction { name, ctx } => match client.invoke_action_async(&name, ctx).await {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error(mineral_log::chain(&e)),
        },
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
        Request::ToggleLove(id) => match client.toggle_love_async(&id).await {
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
        } => {
            client.report_terminal_state(rows, cols, fullscreen);
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
        | Request::DrainTaskEvents
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
        Request::CyclePlayMode => Some("CyclePlayMode"),
        Request::PrevOrRestart => Some("PrevOrRestart"),
        Request::NextSong => Some("NextSong"),
        Request::DaemonInfo => Some("DaemonInfo"),
        Request::InvokeAction { .. } => Some("InvokeAction"),
        Request::StoreGet { .. } => Some("StoreGet"),
        Request::StoreSet { .. } => Some("StoreSet"),
        Request::StoreInc { .. } => Some("StoreInc"),
        Request::ScriptBinds => Some("ScriptBinds"),
        Request::ToggleLove(_) => Some("ToggleLove"),
        Request::QuerySongStats(_) => Some("QuerySongStats"),
        Request::Download(_) => Some("Download"),
    }
}

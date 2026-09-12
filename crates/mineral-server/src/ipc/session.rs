//! 接入会话、验证握手并维护唯一 writer 与有界出口。
//!
//! 同一 client 的播放和队列操作按到达顺序执行；慢查询并发执行，以请求 id 配对。
//! writer 合并当前可用消息后发送，慢 client 只对自己的会话施加背压。

use std::sync::Arc;

use color_eyre::eyre::WrapErr;
use mineral_protocol::{
    CloseReason, MessageBatch, RejectReason, Request, ServerHello, SessionMessage, SessionRequest,
    SessionResult, SocketWire, Wire, WireSink, WireSource,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, mpsc};

use super::dispatch;
use super::registry::{ConnGuard, ConnRegistry};
use super::subscriptions::Subscriptions;
use crate::client::ClientHandle;
use crate::publisher::DomainPublishers;

/// 每连接待发消息队列上限(有界出口)。
const OUTBOUND_CAPACITY: usize = 512;

/// daemon writer 单批最多合并的消息数。
const OUTBOUND_BATCH: usize = 64;

/// 会话处理所需的服务集合。
pub(crate) struct SessionServices {
    /// 业务句柄。
    pub(crate) client: ClientHandle,

    /// 在线连接注册表。
    pub(crate) registry: Arc<ConnRegistry>,

    /// 领域发布器。
    pub(crate) publishers: DomainPublishers,

    /// daemon 级关停通知。
    pub(crate) shutdown: Arc<Notify>,

    /// 新连接接入后的刷新回调。
    pub(crate) on_connect: Arc<dyn Fn() + Send + Sync>,
}

/// Accept loop。返回 `Ok(())` 仅在 listener 被外部关闭时;否则一直循环。
pub(crate) async fn run(
    listener: UnixListener,
    services: Arc<SessionServices>,
) -> color_eyre::Result<()> {
    loop {
        let (stream, _addr) = listener
            .accept()
            .await
            .wrap_err("UnixListener::accept failed")?;
        let conn_id = services.registry.register();
        (services.on_connect)();
        mineral_log::info!(target: "ipc", conn_id, "client connected");
        let client = services.client.for_connection(conn_id);
        let guard = ConnGuard {
            registry: Arc::clone(&services.registry),
            id: conn_id,
            client: client.clone(),
        };
        let services = Arc::clone(&services);
        tokio::spawn(async move {
            // 守卫持有到 task 结束:正常返回与 panic unwind 都从注册表移除。
            let _guard = guard;
            if let Err(e) = handle_connection(stream, conn_id, &services).await {
                mineral_log::warn!(target: "ipc", conn_id, error = mineral_log::chain(&e), "connection ended with error");
            }
        });
    }
}

/// 一条连接的完整生命周期:握手 → 拆半 → 读循环 + 发布泵 → 收尾。
async fn handle_connection(
    stream: UnixStream,
    conn_id: u64,
    services: &SessionServices,
) -> color_eyre::Result<()> {
    let mut wire = Box::new(SocketWire::from_stream(stream));
    let Some(info) = handshake(&mut *wire, &services.client).await? else {
        return Ok(());
    };
    services.registry.set_identity(conn_id, info.clone());
    let (sink, source) = wire.split();
    let (out_tx, out_rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let writer = tokio::spawn(writer_loop(sink, out_rx));
    let result = read_loop(source, &services.client, services, &out_tx).await;
    // client 断开:清本连接的终端上报与 PCM 游标。
    services.client.connection_closed();
    drop(out_tx);
    // writer 在全部发送端放掉后自行排空退出;不无限等待(慢 client 的写可能阻塞)。
    writer.abort();
    result
}

/// 握手守门:期待首帧 [`SessionMessage::Hello`],版本不匹配回拒绝。
///
/// # Return:
///   `Some(client 身份)` = 握手通过;`None` = 连接就此关闭。
async fn handshake(
    wire: &mut dyn Wire,
    client: &ClientHandle,
) -> color_eyre::Result<Option<mineral_protocol::ClientInfo>> {
    let batch = match wire.recv().await {
        Ok(Some(batch)) => batch,
        Ok(None) => return Ok(None),
        Err(error) => {
            mineral_log::warn!(target: "ipc", error = mineral_log::chain(&error), "握手读失败");
            return Ok(None);
        }
    };
    let info = batch.messages.into_iter().find_map(|message| {
        if let SessionMessage::Hello(info) = message {
            Some(info)
        } else {
            None
        }
    });
    let Some(info) = info else {
        mineral_log::warn!(target: "ipc", "首帧不是 Hello,断开");
        return Ok(None);
    };
    if !info.version_matches() {
        mineral_log::warn!(
            target: "ipc",
            client_version = %info.version,
            "client 版本不匹配,拒绝连接"
        );
        client.record_connection_reject(mineral_stats::RejectReason::VersionMismatch);
        let _ = wire
            .send(MessageBatch {
                messages: vec![
                    SessionMessage::Welcome(ServerHello::reject(RejectReason::VersionMismatch)),
                    SessionMessage::Goodbye(CloseReason::Protocol {
                        detail: "版本不匹配".to_owned(),
                    }),
                ],
            })
            .await;
        return Ok(None);
    }
    wire.send(MessageBatch::one(SessionMessage::Welcome(
        ServerHello::accept(),
    )))
    .await?;
    mineral_log::debug!(target: "ipc", client = %info.name, "handshake accepted");
    Ok(Some(info))
}

/// 唯一 writer:从有界队列取当前可用的消息合成一批,一次 sink send。
async fn writer_loop(mut sink: Box<dyn WireSink>, mut rx: mpsc::Receiver<SessionMessage>) {
    let mut buf = Vec::with_capacity(OUTBOUND_BATCH);
    loop {
        let count = rx.recv_many(&mut buf, OUTBOUND_BATCH).await;
        if count == 0 {
            break;
        }
        let batch = MessageBatch {
            messages: std::mem::take(&mut buf),
        };
        if let Err(error) = sink.send(batch).await {
            mineral_log::warn!(target: "ipc", error = mineral_log::chain(&error), "会话写失败,writer 退出");
            break;
        }
    }
    let _ = sink.close().await;
}

/// 读循环:按到达次序处理请求 / 订阅管理,直到 client 关闭。
async fn read_loop(
    mut source: Box<dyn WireSource>,
    client: &ClientHandle,
    services: &SessionServices,
    out: &mpsc::Sender<SessionMessage>,
) -> color_eyre::Result<()> {
    let mut subscriptions = Subscriptions::default();
    loop {
        let batch = match source.recv().await {
            Ok(Some(batch)) => batch,
            Ok(None) => break,
            Err(error) => {
                mineral_log::warn!(target: "ipc", error = mineral_log::chain(&error), "会话读失败");
                break;
            }
        };
        for message in batch.messages {
            match message {
                SessionMessage::Requests(requests) => {
                    for request in requests {
                        handle_request(request, client, out, &services.shutdown).await;
                    }
                }
                SessionMessage::Subscribe(request) => {
                    subscriptions.subscribe(&request, client, services, out);
                }
                SessionMessage::Unsubscribe(id) => {
                    subscriptions.unsubscribe(id, services.publishers.pcm());
                }
                SessionMessage::Resync(id) => subscriptions.resync(id),
                SessionMessage::Close(_reason) => {
                    subscriptions.shutdown(services.publishers.pcm());
                    return Ok(());
                }
                other => {
                    mineral_log::warn!(target: "ipc", message = ?other, "忽略意外会话消息");
                }
            }
        }
    }
    subscriptions.shutdown(services.publishers.pcm());
    Ok(())
}

/// 处理一条请求:有序同步内联;队列编辑内联 await;慢操作并发。
async fn handle_request(
    request: SessionRequest,
    client: &ClientHandle,
    out: &mpsc::Sender<SessionMessage>,
    shutdown: &Arc<Notify>,
) {
    let id = request.id;
    if let Some(slow) = dispatch::async_request(&request.request) {
        let client = client.clone();
        let out = out.clone();
        tokio::spawn(async move {
            let result = dispatch::execute_async(&client, slow).await;
            let _ = out
                .send(SessionMessage::Results(vec![SessionResult { id, result }]))
                .await;
        });
        return;
    }
    let result = match request.request {
        Request::QueueEdit { op } => dispatch::execute_queue_edit(client, op).await,
        other => {
            let is_shutdown = matches!(other, Request::Shutdown);
            let result = dispatch::execute_sync(client, other);
            if is_shutdown {
                // ack 先入队、再唤醒关停:给 writer 把应答写出去的机会。
                let _ = out
                    .send(SessionMessage::Results(vec![SessionResult {
                        id,
                        result: result.clone(),
                    }]))
                    .await;
                shutdown.notify_one();
                return;
            }
            result
        }
    };
    let _ = out
        .send(SessionMessage::Results(vec![SessionResult { id, result }]))
        .await;
}

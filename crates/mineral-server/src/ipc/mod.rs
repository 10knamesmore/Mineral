//! daemon 侧会话处理:accept loop、握手守门、唯一 writer、按订阅主题的发布泵。
//!
//! - 连接上跑的是 [`mineral_protocol::SessionMessage`],请求成批到达、结果与更新交错。
//! - **有序提交**:同一 client 的播放 / 队列操作在 read loop 内按到达次序执行;
//!   慢查询与脚本 / 数据库操作并发执行,结果经请求 id 配对。
//! - **共享发布**:每个订阅主题一条泵,消费领域发布器的 `watch` / `broadcast`,
//!   按订阅自己的版本转发,不按连接重建整份状态。
//! - **有界出口**:唯一 writer 从有界队列取当前可用的消息合成一批,慢 client 以
//!   `await` 背压自己的会话,不拖累其他连接。

mod dispatch;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use color_eyre::eyre::WrapErr;
use mineral_protocol::{
    CloseReason, DownloadDetailDelta, MessageBatch, PlayerVersions, RejectReason, Request,
    ServerHello, SessionMessage, SessionRequest, SessionResult, SocketWire, SubscribeRequest,
    SubscriptionId, SubscriptionTopic, UpdateEnvelope, UpdatePayload, Wire, WireSink, WireSource,
    fragment_download_detail, fragment_player_update,
};
use rustc_hash::FxHashMap;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::client::ClientHandle;
use crate::pcm_relay::{PcmRelay, PcmSubscription};
use crate::publisher::DomainPublishers;

/// 每连接待发消息队列上限(有界出口)。
const OUTBOUND_CAPACITY: usize = 512;

/// daemon writer 单批最多合并的消息数。
const OUTBOUND_BATCH: usize = 64;

/// 已接入连接的注册表:accept 分配自增 id、断开移除;心跳读在线数,握手后
/// 登记 client 身份(版本 + 自报名)。
pub(crate) struct ConnRegistry {
    /// 下一个连接 id(进程内单调自增,不复用)。
    next_id: AtomicU64,

    /// 在线连接:id → 连接元数据。
    conns: parking_lot::Mutex<rustc_hash::FxHashMap<u64, ConnMeta>>,
}

/// 一条在线连接的元数据(断开时结算 client_connections 埋点用)。
struct ConnMeta {
    /// 握手身份(握手完成前为 `None`;埋点只结算握手完成的连接)。
    identity: Option<mineral_protocol::ClientInfo>,

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
    fn set_identity(&self, id: u64, info: mineral_protocol::ClientInfo) {
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
/// 并结算 client_connections 埋点。
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

/// 订阅泵句柄。
struct Pump {
    /// 泵任务。
    handle: JoinHandle<()>,

    /// 重同步唤醒(Player / DownloadsDetail 用)。
    resync: Arc<Notify>,

    /// PCM 订阅令牌(退订时归还中继)。
    pcm: Option<PcmSubscription>,
}

/// 一条连接的订阅登记。
#[derive(Default)]
struct Subscriptions {
    /// 订阅 id → 泵。
    pumps: FxHashMap<SubscriptionId, Pump>,
}

impl Subscriptions {
    /// 启动一个订阅泵(重复 id 直接忽略)。
    ///
    /// # Params:
    ///   - `request`: 订阅请求
    ///   - `client`: 业务句柄
    ///   - `services`: 服务集合
    ///   - `out`: 本连接出口
    fn subscribe(
        &mut self,
        request: &SubscribeRequest,
        client: &ClientHandle,
        services: &SessionServices,
        out: &mpsc::Sender<SessionMessage>,
    ) {
        if self.pumps.contains_key(&request.id) {
            mineral_log::warn!(
                target: "ipc",
                subscription = request.id.value(),
                "重复订阅 id,忽略"
            );
            return;
        }
        let resync = Arc::new(Notify::new());
        let (handle, pcm) = match request.topic {
            SubscriptionTopic::Player => {
                let known = request.known_player.unwrap_or_default();
                let handle = tokio::spawn(pump_player(
                    client.clone(),
                    request.id,
                    known,
                    Arc::clone(&resync),
                    out.clone(),
                ));
                (handle, None)
            }
            SubscriptionTopic::Playback => {
                let handle = tokio::spawn(pump_playback(
                    services.publishers.playback(),
                    request.id,
                    out.clone(),
                ));
                (handle, None)
            }
            SubscriptionTopic::Tasks => {
                let handle = tokio::spawn(pump_tasks(
                    services.publishers.tasks(),
                    request.id,
                    out.clone(),
                ));
                (handle, None)
            }
            SubscriptionTopic::DownloadsSummary => {
                let handle = tokio::spawn(pump_downloads_summary(
                    services.publishers.downloads_summary(),
                    request.id,
                    out.clone(),
                ));
                (handle, None)
            }
            SubscriptionTopic::DownloadsDetail => {
                let handle = tokio::spawn(pump_downloads_detail(
                    services.publishers.downloads_detail(),
                    services.publishers.downloads_delta(),
                    request.id,
                    Arc::clone(&resync),
                    out.clone(),
                ));
                (handle, None)
            }
            SubscriptionTopic::Pcm => {
                let (token, rx) = services.publishers.pcm().subscribe();
                let handle = tokio::spawn(pump_pcm(rx, request.id, out.clone()));
                (handle, Some(token))
            }
            SubscriptionTopic::Events(category) => {
                let handle = tokio::spawn(pump_events(
                    services.publishers.events(),
                    category,
                    request.id,
                    client.clone(),
                    out.clone(),
                ));
                (handle, None)
            }
        };
        self.pumps.insert(
            request.id,
            Pump {
                handle,
                resync,
                pcm,
            },
        );
    }

    /// 取消订阅。
    ///
    /// # Params:
    ///   - `id`: 订阅 id
    ///   - `pcm`: PCM 中继(归还游标)
    fn unsubscribe(&mut self, id: SubscriptionId, pcm: &PcmRelay) {
        let Some(pump) = self.pumps.remove(&id) else {
            return;
        };
        pump.handle.abort();
        if let Some(token) = pump.pcm {
            pcm.unsubscribe(token);
        }
    }

    /// 请求重新快照。
    ///
    /// # Params:
    ///   - `id`: 订阅 id
    fn resync(&self, id: SubscriptionId) {
        if let Some(pump) = self.pumps.get(&id) {
            pump.resync.notify_one();
        }
    }

    /// 连接收尾:停掉全部泵并归还 PCM 游标。
    ///
    /// # Params:
    ///   - `pcm`: PCM 中继
    fn shutdown(&mut self, pcm: &PcmRelay) {
        for (_id, pump) in self.pumps.drain() {
            pump.handle.abort();
            if let Some(token) = pump.pcm {
                pcm.unsubscribe(token);
            }
        }
    }
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

/// 播放订阅泵:订阅即发一帧(版本门控),之后按领域变更推增量。
async fn pump_player(
    client: ClientHandle,
    id: SubscriptionId,
    mut known: PlayerVersions,
    resync: Arc<Notify>,
    out: mpsc::Sender<SessionMessage>,
) {
    let mut changes = client.state_changes();
    let mut version = 0_u64;
    loop {
        let sync = client.player_sync(known);
        known = sync.versions;
        version = version.saturating_add(1);
        for message in fragment_player_update(id, version, sync) {
            if out.send(message).await.is_err() {
                return;
            }
        }
        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = resync.notified() => {
                // 重同步:清已知版本并从版本 1 重发完整快照(与 client 重置基线配对)。
                known = PlayerVersions::default();
                version = 0;
            }
        }
    }
}

/// 单值 watch 泵:订阅即发当前值,变化即发(积压由 watch 合并为最新值)。
async fn pump_watch<T, F>(
    mut rx: watch::Receiver<T>,
    id: SubscriptionId,
    out: mpsc::Sender<SessionMessage>,
    wrap: F,
) where
    T: Clone + Send + Sync + 'static,
    F: Fn(T) -> UpdatePayload + Send + 'static,
{
    let mut version = 0_u64;
    loop {
        let value = rx.borrow_and_update().clone();
        version = version.saturating_add(1);
        let message = SessionMessage::Update(UpdateEnvelope {
            subscription: id,
            version,
            parts: 1,
            index: 0,
            payload: wrap(value),
        });
        if out.send(message).await.is_err() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// 播放锚点泵。
async fn pump_playback(
    rx: watch::Receiver<Arc<mineral_audio::AudioSnapshot>>,
    id: SubscriptionId,
    out: mpsc::Sender<SessionMessage>,
) {
    pump_watch(rx, id, out, |snapshot| {
        UpdatePayload::Playback(Box::new(*snapshot.as_ref()))
    })
    .await;
}

/// 任务摘要泵。
async fn pump_tasks(
    rx: watch::Receiver<Arc<mineral_task::Snapshot>>,
    id: SubscriptionId,
    out: mpsc::Sender<SessionMessage>,
) {
    pump_watch(rx, id, out, |tasks| {
        UpdatePayload::Tasks(Box::new(tasks.as_ref().clone()))
    })
    .await;
}

/// 下载摘要泵。
async fn pump_downloads_summary(
    rx: watch::Receiver<Arc<mineral_protocol::DownloadSummary>>,
    id: SubscriptionId,
    out: mpsc::Sender<SessionMessage>,
) {
    pump_watch(rx, id, out, |summary| {
        UpdatePayload::DownloadsSummary(summary.as_ref().clone())
    })
    .await;
}

/// 下载明细泵:先发完整快照,再转发增量;Lagged / Resync 时重发快照。
async fn pump_downloads_detail(
    mut snapshot: watch::Receiver<Option<Arc<Vec<mineral_protocol::SongDownloadView>>>>,
    mut deltas: broadcast::Receiver<Arc<DownloadDetailDelta>>,
    id: SubscriptionId,
    resync: Arc<Notify>,
    out: mpsc::Sender<SessionMessage>,
) {
    let mut version = 0_u64;
    loop {
        let rows = snapshot
            .borrow_and_update()
            .as_ref()
            .map_or_else(Vec::new, |rows| rows.as_ref().clone());
        version = version.saturating_add(1);
        for message in fragment_download_detail(id, version, rows) {
            if out.send(message).await.is_err() {
                return;
            }
        }
        let reason = tokio::select! {
            result = deltas.recv() => match result {
                Ok(delta) => {
                    version = version.saturating_add(1);
                    let message = SessionMessage::Update(UpdateEnvelope {
                        subscription: id,
                        version,
                        parts: 1,
                        index: 0,
                        payload: UpdatePayload::DownloadsDetailDelta(delta.as_ref().clone()),
                    });
                    if out.send(message).await.is_err() {
                        return;
                    }
                    None
                }
                Err(broadcast::error::RecvError::Lagged(_skipped)) => Some(ResyncSource::Daemon),
                Err(broadcast::error::RecvError::Closed) => return,
            },
            changed = snapshot.changed() => {
                if changed.is_err() { return; }
                Some(ResyncSource::Daemon)
            }
            () = resync.notified() => Some(ResyncSource::Client),
        };
        if let Some(source) = reason {
            // 仅 client 请求重同步时重置版本基线(它同步清空了已应用版本);
            // daemon 侧重建快照继续递增,client 以非增量载荷直接采纳。
            if matches!(source, ResyncSource::Client) {
                version = 0;
            }
            continue;
        }
    }
}

/// 下载明细泵重发完整快照的触发方。
#[derive(Clone, Copy)]
enum ResyncSource {
    /// client 显式请求(版本基线由 client 同步重置)。
    Client,

    /// daemon 侧增量积压 / 快照重建(版本继续递增)。
    Daemon,
}

/// PCM 订阅泵:把中继切好的块转发到会话出口(有界)。
async fn pump_pcm(
    mut rx: mpsc::Receiver<mineral_protocol::PcmChunk>,
    id: SubscriptionId,
    out: mpsc::Sender<SessionMessage>,
) {
    let mut version = 0_u64;
    while let Some(chunk) = rx.recv().await {
        version = version.saturating_add(1);
        let message = SessionMessage::Update(UpdateEnvelope {
            subscription: id,
            version,
            parts: 1,
            index: 0,
            payload: UpdatePayload::Pcm(chunk),
        });
        if out.send(message).await.is_err() {
            return;
        }
    }
}

/// 事件类别泵:订阅即重放当前可重放快照(配置 / 窗口标题 / 任务库);之后按类别
/// 过滤广播;Lagged 时再补发一次当前快照。
async fn pump_events(
    mut rx: broadcast::Receiver<mineral_protocol::Event>,
    category: mineral_protocol::Subscription,
    id: SubscriptionId,
    client: ClientHandle,
    out: mpsc::Sender<SessionMessage>,
) {
    let mut version = 0_u64;
    let replay = client.replay_frames(&[category]).await;
    if send_events(&replay, id, &mut version, &out).await.is_err() {
        return;
    }
    loop {
        match rx.recv().await {
            Ok(event) => {
                if event.subscription() != category {
                    continue;
                }
                if send_events(&[event], id, &mut version, &out).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                mineral_log::warn!(
                    target: "ipc",
                    category = ?category,
                    skipped,
                    "事件积压,尝试补发当前快照"
                );
                let replay = client.replay_frames(&[category]).await;
                if send_events(&replay, id, &mut version, &out).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// 把一批事件按当前版本号连续发出;发送失败返回 `Err`。
///
/// # Params:
///   - `events`: 待发事件
///   - `id`: 目标订阅
///   - `version`: 该订阅的版本计数器(就地递增)
///   - `out`: 会话出口
async fn send_events(
    events: &[mineral_protocol::Event],
    id: SubscriptionId,
    version: &mut u64,
    out: &mpsc::Sender<SessionMessage>,
) -> Result<(), ()> {
    for event in events {
        *version = version.saturating_add(1);
        let message = SessionMessage::Update(UpdateEnvelope {
            subscription: id,
            version: *version,
            parts: 1,
            index: 0,
            payload: UpdatePayload::Event(Box::new(event.clone())),
        });
        if out.send(message).await.is_err() {
            return Err(());
        }
    }
    Ok(())
}

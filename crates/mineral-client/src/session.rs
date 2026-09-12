//! 客户端会话核心:请求入有界 channel、结果配对与分流、自动合批、独立 reader/writer、
//! 订阅分片组装与断连收束。
//!
//! 设计要点:
//! - **不凑批**:writer 等到第一件事后只吸干当前已排队的命令,不设固定攒批窗口。
//! - **先登记再发送**:请求的结果去向在写线前进入在途表,慢 writer 不阻塞 reader。
//! - **有界**:待发命令、在途请求、事件缓冲、PCM 窗口、分片组装各自限额;超限明确失败。
//! - **断连即收束**:等待结论的调用方得到「结果未知」,不伪造业务默认值,也不自动重发。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use mineral_protocol::{
    ClientInfo, CloseReason, HandshakeRejected, MessageBatch, PlayerVersions, Request, RequestId,
    SessionMessage, SessionRequest, SessionResult, SubscribeRequest, SubscriptionId,
    SubscriptionTopic, UpdateEnvelope, UpdatePayload, Wire, WireSink, WireSource, assembly_limits,
};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::connection::{ClientConfig, ConnectError, SessionMetrics};
use crate::operation::SubmitError;
use crate::state::{ApplyOutcome, Mirror};

mod results;

pub(crate) use results::ResultTarget;

/// 会话内部共享状态。
pub(crate) struct SessionShared {
    /// client 侧镜像。
    pub(crate) mirror: Arc<Mirror>,

    /// 链路是否可用。
    connected: AtomicBool,

    /// 在途请求 → 结果处理去向。
    inflight: Mutex<FxHashMap<RequestId, ResultTarget>>,

    /// 在途计数(含已入队未发送)。
    in_flight: AtomicUsize,

    /// 已发送批次数。
    batches: AtomicU64,

    /// 已发送消息数。
    messages: AtomicU64,

    /// 已发送请求数。
    requests: AtomicU64,

    /// 因容量丢弃的更新数。
    dropped: AtomicU64,

    /// 关闭信号(reader / writer 互相收束)。
    cancel: CancellationToken,
}

impl SessionShared {
    /// 构造空共享态。
    ///
    /// # Params:
    ///   - `mirror`: client 侧镜像
    fn new(mirror: Arc<Mirror>) -> Self {
        Self {
            mirror,
            connected: AtomicBool::new(true),
            inflight: Mutex::new(FxHashMap::default()),
            in_flight: AtomicUsize::new(0),
            batches: AtomicU64::new(0),
            messages: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            cancel: CancellationToken::new(),
        }
    }

    /// 链路是否可用。
    pub(crate) fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// 当前指标。
    pub(crate) fn metrics(&self) -> SessionMetrics {
        SessionMetrics {
            batches_sent: self.batches.load(Ordering::Relaxed),
            messages_sent: self.messages.load(Ordering::Relaxed),
            requests_sent: self.requests.load(Ordering::Relaxed),
            updates_dropped: self.dropped.load(Ordering::Relaxed),
        }
    }

    /// 断连收束:置断连、唤醒 writer/reader、清除在途记录(等待者收到「结果未知」)。
    fn disconnect(&self) {
        self.connected.store(false, Ordering::Release);
        self.mirror.set_connected(false);
        self.cancel.cancel();
        self.inflight.lock().clear();
        self.in_flight.store(0, Ordering::Release);
    }
}

/// 会话命令(进入 writer 的有界队列)。
enum Command {
    /// 一条请求及其结果处理去向。
    Request {
        /// 会话内配对 id。
        id: RequestId,

        /// 业务请求。
        request: Request,

        /// 收到结论后交给等待者，或由会话记录业务失败。
        reply: ResultTarget,
    },

    /// 建立订阅。
    Subscribe(SubscribeRequest),

    /// 取消订阅。
    Unsubscribe(SubscriptionId),

    /// 请求重新快照。
    Resync(SubscriptionId),

    /// 关闭会话。
    Close(CloseReason),
}

/// 会话句柄(持有方负责 clone;命令经有界队列交给 writer)。
pub(crate) struct SessionHandle {
    /// 命令队列发送端。
    tx: mpsc::Sender<Command>,

    /// 共享态。
    shared: Arc<SessionShared>,

    /// 下一个请求 id。
    next_request: AtomicU64,

    /// 下一个订阅 id。
    next_subscription: AtomicU64,
}

impl SessionHandle {
    /// 分配请求 id 并投递请求;先占在途额度,避免无限排队。
    ///
    /// # Params:
    ///   - `request`: 业务请求
    ///   - `reply`: 操作结论的处理去向
    ///   - `max_in_flight`: 未完成请求上限(含已入队未发送)
    ///
    /// # Return:
    ///   已入本地队列的请求 id(不表示已发送 / 已执行);结果按 `reply` 处理。
    ///
    /// # Errors
    /// 队列满 / 在途超限 / 会话已断开。
    pub(crate) fn submit(
        &self,
        request: Request,
        reply: ResultTarget,
        max_in_flight: usize,
    ) -> Result<RequestId, SubmitError> {
        if !self.shared.connected() {
            return Err(SubmitError::Disconnected);
        }
        let previous = self.shared.in_flight.fetch_add(1, Ordering::AcqRel);
        if previous >= max_in_flight {
            self.shared.in_flight.fetch_sub(1, Ordering::AcqRel);
            return Err(SubmitError::InFlightLimit);
        }
        let id = RequestId::new(self.next_request.fetch_add(1, Ordering::Relaxed));
        match self.tx.try_send(Command::Request { id, request, reply }) {
            Ok(()) => Ok(id),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.shared.in_flight.fetch_sub(1, Ordering::AcqRel);
                Err(SubmitError::QueueFull)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.shared.in_flight.fetch_sub(1, Ordering::AcqRel);
                Err(SubmitError::Disconnected)
            }
        }
    }

    /// 建立订阅(先登记镜像主题,再发命令,早到的更新不会被误丢)。
    ///
    /// # Params:
    ///   - `topic`: 订阅主题
    ///   - `known_player`: Player 主题的已知播放版本
    ///
    /// # Return:
    ///   订阅 id。
    pub(crate) fn subscribe(
        &self,
        topic: SubscriptionTopic,
        known_player: Option<PlayerVersions>,
    ) -> SubscriptionId {
        let id = SubscriptionId::new(self.next_subscription.fetch_add(1, Ordering::Relaxed));
        self.shared.mirror.register_subscription(id, topic);
        let request = SubscribeRequest {
            id,
            topic,
            known_player,
        };
        if self.tx.try_send(Command::Subscribe(request)).is_err() {
            self.shared.mirror.unregister_subscription(id);
        }
        id
    }

    /// 取消订阅。
    ///
    /// # Params:
    ///   - `id`: 订阅 id
    pub(crate) fn unsubscribe(&self, id: SubscriptionId) {
        self.shared.mirror.unregister_subscription(id);
        let _ = self.tx.try_send(Command::Unsubscribe(id));
    }

    /// 主动关闭会话(尽力送出 Close,然后收束)。
    pub(crate) fn close(&self) {
        // 交给 writer 先发 Close 再退;只有在队列满时才直接取消(尽力语义)。
        if self
            .tx
            .try_send(Command::Close(CloseReason::ClientClosed))
            .is_err()
        {
            self.shared.cancel.cancel();
        }
    }
}

/// 建立会话:发 Hello、等 Welcome、校验版本,然后拆半驱动 reader / writer。
///
/// # Params:
///   - `wire`: 已连接的承载(未握手)
///   - `name`: client 自报名(埋点归属)
///   - `config`: 容量参数
///
/// # Errors
/// 传输失败 / 握手被拒 / 协议违规。
pub(crate) async fn open_session(
    mut wire: Box<dyn Wire>,
    name: &str,
    config: ClientConfig,
) -> Result<SessionHandle, ConnectError> {
    wire.send(MessageBatch::one(SessionMessage::Hello(ClientInfo::new(
        name,
    ))))
    .await?;
    let welcome = match wire.recv().await? {
        Some(batch) => batch.messages.into_iter().find_map(|message| {
            if let SessionMessage::Welcome(hello) = message {
                Some(hello)
            } else {
                None
            }
        }),
        None => {
            return Err(ConnectError::Protocol {
                detail: "daemon 在握手期间关闭了连接".to_owned(),
            });
        }
    };
    let Some(hello) = welcome else {
        return Err(ConnectError::Protocol {
            detail: "握手首答不是 Welcome".to_owned(),
        });
    };
    if let Err(rejected) = hello.ensure_accepted() {
        // 握手被拒后关闭承载,不启动会话读写任务。
        let _ = wire.close().await;
        return Err(match rejected.downcast::<HandshakeRejected>() {
            Ok(rejected) => ConnectError::Rejected(rejected),
            Err(other) => ConnectError::Protocol {
                detail: format!("{other:#}"),
            },
        });
    }

    let mirror = Arc::new(Mirror::new(config.event_capacity, config.pcm_window));
    let shared = Arc::new(SessionShared::new(Arc::clone(&mirror)));
    let (tx, rx) = mpsc::channel(config.outbound_capacity.max(1));
    let (resync_tx, resync_rx) = mpsc::unbounded_channel();
    let (sink, source) = wire.split();
    let writer = tokio::spawn(writer_loop(
        sink,
        rx,
        resync_rx,
        Arc::clone(&shared),
        config,
    ));
    let reader = tokio::spawn(reader_loop(source, Arc::clone(&shared), resync_tx));
    // 丢弃 JoinHandle 让读写任务独立运行;任一侧断连都会通知另一侧收尾。
    drop(writer);
    drop(reader);
    Ok(SessionHandle {
        tx,
        shared,
        next_request: AtomicU64::new(0),
        next_subscription: AtomicU64::new(0),
    })
}

/// writer:吸干当前可用命令 → 组装一批消息 → 一次 sink send。不等待凑满、不定时。
async fn writer_loop(
    mut sink: Box<dyn WireSink>,
    mut rx: mpsc::Receiver<Command>,
    mut resync_rx: mpsc::UnboundedReceiver<SubscriptionId>,
    shared: Arc<SessionShared>,
    config: ClientConfig,
) {
    let mut buf = Vec::with_capacity(config.max_batch);
    loop {
        let first = tokio::select! {
            biased;
            () = shared.cancel.cancelled() => break,
            maybe = rx.recv() => match maybe {
                Some(command) => command,
                None => break,
            },
            Some(id) = resync_rx.recv() => Command::Resync(id),
        };
        buf.clear();
        buf.push(first);
        while buf.len() < config.max_batch {
            match rx.try_recv() {
                Ok(command) => buf.push(command),
                Err(_) => break,
            }
        }
        while buf.len() < config.max_batch {
            match resync_rx.try_recv() {
                Ok(id) => buf.push(Command::Resync(id)),
                Err(_) => break,
            }
        }
        let mut close_after = false;
        let batch = build_batch(&mut buf, &shared, &mut close_after);
        let message_count = batch.messages.len();
        if message_count > 0
            && let Err(error) = sink.send(batch).await
        {
            mineral_log::warn!(target: "ipc", error = mineral_log::chain(&error), "会话写失败,收束会话");
            shared.disconnect();
            break;
        }
        shared.batches.fetch_add(1, Ordering::Relaxed);
        shared.messages.fetch_add(
            u64::try_from(message_count).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        if close_after {
            break;
        }
    }
    let _ = sink.close().await;
    shared.disconnect();
}

/// 把一批命令转成会话消息;请求先登记在途表(先登记后发送)。
fn build_batch(
    commands: &mut Vec<Command>,
    shared: &SessionShared,
    close_after: &mut bool,
) -> MessageBatch {
    let mut messages = Vec::new();
    let mut requests: Vec<SessionRequest> = Vec::new();
    for command in commands.drain(..) {
        match command {
            Command::Request { id, request, reply } => {
                shared.inflight.lock().insert(id, reply);
                requests.push(SessionRequest { id, request });
            }
            Command::Subscribe(request) => {
                flush_requests(&mut messages, &mut requests);
                messages.push(SessionMessage::Subscribe(request));
            }
            Command::Unsubscribe(id) => {
                flush_requests(&mut messages, &mut requests);
                messages.push(SessionMessage::Unsubscribe(id));
            }
            Command::Resync(id) => {
                flush_requests(&mut messages, &mut requests);
                messages.push(SessionMessage::Resync(id));
            }
            Command::Close(reason) => {
                flush_requests(&mut messages, &mut requests);
                messages.push(SessionMessage::Close(reason));
                *close_after = true;
            }
        }
    }
    flush_requests(&mut messages, &mut requests);
    let request_count = messages
        .iter()
        .map(|message| match message {
            SessionMessage::Requests(batch) => batch.len(),
            _ => 0,
        })
        .sum::<usize>();
    shared.requests.fetch_add(
        u64::try_from(request_count).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    MessageBatch { messages }
}

/// 把累积的请求合成一条 `Requests` 消息(连续请求合并,保持相对次序)。
fn flush_requests(messages: &mut Vec<SessionMessage>, requests: &mut Vec<SessionRequest>) {
    if requests.is_empty() {
        return;
    }
    messages.push(SessionMessage::Requests(std::mem::take(requests)));
}

/// reader:处理结果 / 更新 / 关闭,并驱动分片组装与镜像应用。
async fn reader_loop(
    mut source: Box<dyn WireSource>,
    shared: Arc<SessionShared>,
    resync_tx: mpsc::UnboundedSender<SubscriptionId>,
) {
    let mut assembler = Assembler::default();
    loop {
        let batch = tokio::select! {
            biased;
            () = shared.cancel.cancelled() => break,
            result = source.recv() => match result {
                Ok(Some(batch)) => batch,
                Ok(None) => {
                    mineral_log::warn!(target: "ipc", "daemon 关闭了连接");
                    break;
                }
                Err(error) => {
                    mineral_log::warn!(target: "ipc", error = mineral_log::chain(&error), "会话读失败,收束会话");
                    break;
                }
            },
        };
        for message in batch.messages {
            match message {
                SessionMessage::Results(results) => {
                    for result in results {
                        resolve_result(&shared, result);
                    }
                }
                SessionMessage::Update(envelope) => {
                    handle_update(&shared, &mut assembler, envelope, &resync_tx);
                }
                SessionMessage::Goodbye(reason) => {
                    mineral_log::warn!(target: "ipc", reason = ?reason, "daemon 结束会话");
                    break;
                }
                SessionMessage::Welcome(_) => {
                    mineral_log::warn!(target: "ipc", "重复 Welcome,忽略");
                }
                other => {
                    mineral_log::warn!(target: "ipc", message = ?other, "忽略意外会话消息");
                }
            }
        }
    }
    shared.disconnect();
}

/// 交付一条请求结果;无主的迟到结果只记日志。
fn resolve_result(shared: &SessionShared, result: SessionResult) {
    let reply = shared.inflight.lock().remove(&result.id);
    let Some(reply) = reply else {
        mineral_log::warn!(target: "ipc", id = result.id.value(), "无主结果,丢弃");
        return;
    };
    shared.in_flight.fetch_sub(1, Ordering::AcqRel);
    reply.deliver(result.id, result.result);
}

/// 处理一条订阅更新(必要时先组装分片)。
fn handle_update(
    shared: &SessionShared,
    assembler: &mut Assembler,
    envelope: UpdateEnvelope,
    resync_tx: &mpsc::UnboundedSender<SubscriptionId>,
) {
    let subscription = envelope.subscription;
    match assembler.accept(envelope) {
        Ok(Some((id, version, payload))) => {
            match shared.mirror.apply_update(id, version, payload) {
                ApplyOutcome::Resync => {
                    // 丢掉旧版本基线:daemon 重同步后从版本 1 重发完整快照。
                    shared.mirror.reset_subscription(id);
                    request_resync(resync_tx, id);
                }
                ApplyOutcome::Applied | ApplyOutcome::Ignored => {}
            }
        }
        Ok(None) => {}
        Err(error) => {
            mineral_log::warn!(target: "ipc", subscription = subscription.value(), error = %error, "订阅分片组装失败");
            shared.dropped.fetch_add(1, Ordering::Relaxed);
            shared.mirror.reset_subscription(subscription);
            request_resync(resync_tx, subscription);
        }
    }
}

/// 经独立通道向 writer 请求重新快照,不占命令队列容量;通道关闭时记警告。
fn request_resync(resync_tx: &mpsc::UnboundedSender<SubscriptionId>, id: SubscriptionId) {
    if resync_tx.send(id).is_err() {
        mineral_log::warn!(target: "ipc", subscription = id.value(), "重同步通道已关闭");
    }
}

/// 分片组装器:按 `(subscription, version)` 收齐 `parts` 后合并成一条逻辑更新。
#[derive(Default)]
struct Assembler {
    /// 进行中的组装组(插入序用于超限时驱逐最旧)。
    groups: Vec<(SubscriptionId, u64, Group)>,
}

/// 一个组装组。
#[derive(Default)]
struct Group {
    /// 各片载荷(按 index 就位)。
    parts: Vec<Option<UpdatePayload>>,

    /// 已到片数。
    received: u32,

    /// 载荷字节估算。
    bytes: usize,
}

impl Assembler {
    /// 丢弃该订阅下所有版本更早的未完成组;返回是否有组被丢弃。
    ///
    /// # Params:
    ///   - `subscription`: 目标订阅
    ///   - `version`: 新到的版本
    fn drop_superseded(&mut self, subscription: SubscriptionId, version: u64) -> bool {
        let before = self.groups.len();
        self.groups
            .retain(|(id, existing, _group)| !(*id == subscription && *existing < version));
        before != self.groups.len()
    }

    /// 接收一条更新信封;完整时返回合并后的 `(subscription, version, payload)`。
    fn accept(
        &mut self,
        envelope: UpdateEnvelope,
    ) -> Result<Option<(SubscriptionId, u64, UpdatePayload)>, String> {
        // 新版本到来 = 旧版本的未完成组再也不会补齐;丢掉并显式报告中断。
        let superseded = self.drop_superseded(envelope.subscription, envelope.version);
        if envelope.parts == 1 {
            return Ok(Some((
                envelope.subscription,
                envelope.version,
                envelope.payload,
            )));
        }
        if superseded {
            return Err("更早版本的未完成分片组已被取代".to_owned());
        }
        if envelope.parts > assembly_limits::MAX_PARTS {
            return Err(format!("分片数 {} 超限", envelope.parts));
        }
        if envelope.index >= envelope.parts {
            return Err(format!(
                "分片序号 {} 超出总片数 {}",
                envelope.index, envelope.parts
            ));
        }
        let key = (envelope.subscription, envelope.version);
        let position = self
            .groups
            .iter()
            .position(|(id, version, _group)| (*id, *version) == key);
        let position = match position {
            Some(position) => position,
            None => {
                if self.groups.len() >= assembly_limits::MAX_GROUPS {
                    // 驱逐最旧组:旧版本不会再被补发,直接放弃并请求重同步。
                    let evicted = self.groups.remove(0);
                    mineral_log::warn!(
                        target: "ipc",
                        subscription = evicted.0.value(),
                        version = evicted.1,
                        "组装组超限,放弃最旧组"
                    );
                }
                let total = usize::try_from(envelope.parts).unwrap_or(usize::MAX);
                self.groups.push((
                    key.0,
                    key.1,
                    Group {
                        parts: vec![None; total],
                        received: 0,
                        bytes: 0,
                    },
                ));
                self.groups.len().saturating_sub(1)
            }
        };
        let index = usize::try_from(envelope.index).unwrap_or(usize::MAX);
        let too_large = {
            let Some((_id, _version, group)) = self.groups.get_mut(position) else {
                return Err("组装组下标越界".to_owned());
            };
            if group.parts.get(index).is_some_and(Option::is_some) {
                return Err(format!("分片 {} 重复到达", envelope.index));
            }
            group.bytes = group
                .bytes
                .saturating_add(estimate_bytes(&envelope.payload));
            if group.bytes > assembly_limits::MAX_GROUP_BYTES {
                true
            } else {
                if let Some(slot) = group.parts.get_mut(index) {
                    *slot = Some(envelope.payload);
                } else {
                    return Err(format!("分片 {} 无处存放", envelope.index));
                }
                group.received = group.received.saturating_add(1);
                false
            }
        };
        if too_large {
            let bytes = self
                .groups
                .get(position)
                .map_or(0, |(_, _, group)| group.bytes);
            self.groups.remove(position);
            return Err(format!("组装字节 {bytes} 超限"));
        }
        let complete = self
            .groups
            .get(position)
            .is_some_and(|(_, _, group)| group.received >= envelope.parts);
        if !complete {
            return Ok(None);
        }
        let (_id, version, group) = self.groups.remove(position);
        let payload = merge_parts(group.parts)?;
        Ok(Some((envelope.subscription, version, payload)))
    }
}

/// 合并一组分片载荷。
fn merge_parts(parts: Vec<Option<UpdatePayload>>) -> Result<UpdatePayload, String> {
    let mut parts = parts
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "组装完成但存在缺片".to_owned())?;
    if parts.len() == 1 {
        return parts.pop().ok_or_else(|| "组装完成但载荷为空".to_owned());
    }
    let first = parts.first().cloned();
    match first {
        Some(UpdatePayload::Player {
            mut sync,
            queue_parts,
        }) => {
            if queue_parts == 0 {
                return Err("播放头段声明无队列分片却出现多片".to_owned());
            }
            let mut queue: Vec<mineral_model::Song> = Vec::new();
            let mut original_queue = None;
            let mut chunks: Vec<(
                u32,
                Vec<mineral_model::Song>,
                Option<Vec<mineral_model::Song>>,
            )> = Vec::new();
            for payload in parts.drain(1..) {
                match payload {
                    UpdatePayload::PlayerQueuePart {
                        offset,
                        queue,
                        original_queue: original,
                    } => {
                        if original.is_some() {
                            original_queue = original;
                        }
                        chunks.push((offset, queue, None));
                    }
                    other => {
                        return Err(format!("播放分片组出现意外载荷 {other:?}"));
                    }
                }
            }
            chunks.sort_by_key(|(offset, _, _)| *offset);
            for (_, chunk, _) in chunks {
                queue.extend(chunk);
            }
            sync.queue = Some(mineral_protocol::QueueSync {
                queue,
                original_queue,
            });
            Ok(UpdatePayload::Player { sync, queue_parts })
        }
        Some(UpdatePayload::DownloadsDetailHead { .. }) => {
            let mut rows = Vec::new();
            let mut chunks: Vec<(u32, Vec<mineral_protocol::SongDownloadView>)> = Vec::new();
            for payload in parts.drain(1..) {
                match payload {
                    UpdatePayload::DownloadsDetailPart { offset, rows: part } => {
                        chunks.push((offset, part));
                    }
                    other => {
                        return Err(format!("下载明细分片组出现意外载荷 {other:?}"));
                    }
                }
            }
            chunks.sort_by_key(|(offset, _)| *offset);
            for (_, chunk) in chunks {
                rows.extend(chunk);
            }
            Ok(UpdatePayload::DownloadsDetailSnapshot(rows))
        }
        Some(other) => Err(format!("不支持的组装载荷 {other:?}")),
        None => Err("组装完成但载荷为空".to_owned()),
    }
}

/// 载荷字节估算(只用于组装上限守卫,不要求精确)。
fn estimate_bytes(payload: &UpdatePayload) -> usize {
    match payload {
        UpdatePayload::Player { sync, .. } => sync
            .queue
            .as_ref()
            .map_or(256, |queue| queue.queue.len().saturating_mul(256)),
        UpdatePayload::PlayerQueuePart { queue, .. } => queue.len().saturating_mul(256),
        UpdatePayload::DownloadsDetailSnapshot(rows) => rows.len().saturating_mul(512),
        UpdatePayload::DownloadsDetailPart { rows, .. } => rows.len().saturating_mul(512),
        UpdatePayload::Pcm(chunk) => chunk.samples.len().saturating_mul(4),
        _ => 512,
    }
}

/// 客户端会话:句柄 + 镜像 + 订阅登记。
pub(crate) struct Session {
    /// 会话句柄。
    pub(crate) handle: SessionHandle,

    /// 容量参数(供 `submit` 判定在途上限)。
    pub(crate) config: ClientConfig,
}

impl Session {
    /// 打开会话。
    ///
    /// # Params:
    ///   - `wire`: 已连接的承载
    ///   - `name`: client 自报名
    ///   - `config`: 容量参数
    ///
    /// # Errors
    /// 握手 / 传输失败。
    pub(crate) async fn open(
        wire: Box<dyn Wire>,
        name: &str,
        config: ClientConfig,
    ) -> Result<Self, ConnectError> {
        let handle = open_session(wire, name, config).await?;
        Ok(Self { handle, config })
    }

    /// 镜像。
    pub(crate) fn mirror(&self) -> &Arc<Mirror> {
        &self.handle.shared.mirror
    }

    /// 链路是否可用。
    pub(crate) fn connected(&self) -> bool {
        self.handle.shared.connected()
    }

    /// 运行指标。
    pub(crate) fn metrics(&self) -> SessionMetrics {
        self.handle.shared.metrics()
    }
}

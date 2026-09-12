//! 会话握手、命令提交、共享状态与断连收束。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use mineral_protocol::{
    ClientInfo, CloseReason, HandshakeRejected, MessageBatch, PlayerVersions, Request, RequestId,
    SessionMessage, SubscribeRequest, SubscriptionId, SubscriptionTopic, Wire,
};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::reader::reader_loop;
use super::results::ResultTarget;
use super::writer::writer_loop;
use crate::connection::{ClientConfig, ConnectError, SessionMetrics};
use crate::operation::SubmitError;
use crate::state::Mirror;

/// 会话内部共享状态。
pub(crate) struct SessionShared {
    /// client 侧镜像。
    pub(crate) mirror: Arc<Mirror>,

    /// 链路是否可用。
    connected: AtomicBool,

    /// 在途请求 → 结果处理去向。
    pub(super) inflight: Mutex<FxHashMap<RequestId, ResultTarget>>,

    /// 在途计数(含已入队未发送)。
    pub(super) in_flight: AtomicUsize,

    /// 已发送批次数。
    pub(super) batches: AtomicU64,

    /// 已发送消息数。
    pub(super) messages: AtomicU64,

    /// 已发送请求数。
    pub(super) requests: AtomicU64,

    /// 因容量丢弃的更新数。
    pub(super) dropped: AtomicU64,

    /// 关闭信号(reader / writer 互相收束)。
    pub(super) cancel: CancellationToken,
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
    pub(super) fn disconnect(&self) {
        self.connected.store(false, Ordering::Release);
        self.mirror.set_connected(false);
        self.cancel.cancel();
        self.inflight.lock().clear();
        self.in_flight.store(0, Ordering::Release);
    }
}

/// 会话命令(进入 writer 的有界队列)。
pub(super) enum Command {
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

/// 客户端会话:句柄 + 镜像 + 订阅登记。
pub(crate) struct Session {
    /// 会话句柄。
    pub(crate) handle: super::SessionHandle,

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
        let handle = super::open_session(wire, name, config).await?;
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

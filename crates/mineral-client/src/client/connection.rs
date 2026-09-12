//! 会话连接、请求提交、引用计数订阅与镜像入口。

use std::sync::Arc;
use std::time::Duration;

use mineral_protocol::{OperationResult, PlayerVersions, SubscriptionId, SubscriptionTopic, Wire};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::oneshot;

use crate::connection::{ClientConfig, ConnectError, SessionMetrics};
use crate::operation::{Outcome, Pending, SubmitError};
use crate::session::{ResultTarget, Session};
use crate::state::Mirror;

/// 订阅引用计数表。
#[derive(Default)]
struct Subscriptions {
    /// 主题 → (订阅 id, 引用数)。
    active: FxHashMap<SubscriptionTopic, (SubscriptionId, usize)>,
}

/// Mineral client:一条逻辑会话 + 一份本地镜像。
pub struct Client {
    /// 会话核心。
    session: Session,

    /// 订阅引用计数。
    subscriptions: Mutex<Subscriptions>,
}

impl Client {
    /// 在已连接的承载上完成握手并启动会话读写。
    ///
    /// # Params:
    ///   - `wire`: 由调用方建立连接、尚未进行会话握手的承载
    ///   - `name`: client 自报名(埋点归属)
    ///   - `config`: 容量参数
    ///
    /// # Errors
    /// 传输失败 / 握手被拒 / 协议违规。
    pub async fn from_wire(
        wire: Box<dyn Wire>,
        name: &str,
        config: ClientConfig,
    ) -> Result<Self, ConnectError> {
        let session = Session::open(wire, name, config).await?;
        Ok(Self {
            session,
            subscriptions: Mutex::new(Subscriptions::default()),
        })
    }

    /// 本地状态镜像。
    #[must_use]
    pub fn mirror(&self) -> &Arc<Mirror> {
        self.session.mirror()
    }

    /// 链路是否可用。
    #[must_use]
    pub fn connected(&self) -> bool {
        self.session.connected()
    }

    /// 会话运行指标(汇总)。
    #[must_use]
    pub fn metrics(&self) -> SessionMetrics {
        self.session.metrics()
    }

    /// 订阅主题(引用计数;同一主题重复订阅返回同一 id)。
    ///
    /// # Params:
    ///   - `topic`: 订阅主题
    ///
    /// # Return:
    ///   会话内订阅 id。
    pub fn subscribe(&self, topic: SubscriptionTopic) -> SubscriptionId {
        let mut subscriptions = self.subscriptions.lock();
        if let Some((id, count)) = subscriptions.active.get_mut(&topic) {
            *count = count.saturating_add(1);
            return *id;
        }
        let known = self.mirror().read_player(|player| {
            (player.versions() != PlayerVersions::default()).then(|| player.versions())
        });
        let id = self.session.handle.subscribe(topic, known);
        subscriptions.active.insert(topic, (id, 1));
        id
    }

    /// 退订主题(引用计数归零才真正发 Unsubscribe)。
    ///
    /// # Params:
    ///   - `topic`: 订阅主题
    pub fn unsubscribe(&self, topic: SubscriptionTopic) {
        let mut subscriptions = self.subscriptions.lock();
        let Some((id, count)) = subscriptions.active.get_mut(&topic) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count > 0 {
            return;
        }
        let id = *id;
        subscriptions.active.remove(&topic);
        drop(subscriptions);
        self.session.handle.unsubscribe(id);
    }

    /// 关闭会话。
    pub fn close(&self) {
        self.session.handle.close();
    }

    /// 等待当前全部订阅收到首帧(CLI 诊断 / 自举用);超时或断连立即返回。
    ///
    /// # Params:
    ///   - `timeout`: 最长等待
    pub async fn wait_subscriptions_ready(&self, timeout: Duration) {
        let ids = {
            let subscriptions = self.subscriptions.lock();
            subscriptions
                .active
                .values()
                .map(|(id, _count)| *id)
                .collect::<Vec<_>>()
        };
        let mirror = Arc::clone(self.mirror());
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if !self.connected() || ids.iter().all(|id| mirror.subscription_seen(*id)) {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// 提交一条请求并等待结论。
    pub(super) async fn request<T>(
        &self,
        request: mineral_protocol::Request,
        decode: impl FnOnce(OperationResult, &'static str) -> Outcome<T> + Send + 'static,
    ) -> Outcome<T> {
        match self.submit(request, decode) {
            Ok(pending) => pending.outcome().await,
            Err(error) => Outcome::Unknown {
                detail: format!("未提交:{error}"),
            },
        }
    }

    /// 提交一条请求,返回结果句柄。
    ///
    /// # Params:
    ///   - `request`: 业务请求
    ///   - `decode`: 依次接收 daemon 结论与从请求派生的 snake_case 名称，转换为操作结论。
    ///
    /// # Errors
    /// 本地未提交(队列满 / 在途超限 / 已断开)。
    pub fn submit<T>(
        &self,
        request: mineral_protocol::Request,
        decode: impl FnOnce(OperationResult, &'static str) -> Outcome<T> + Send + 'static,
    ) -> Result<Pending<T>, SubmitError> {
        let request_name = <&'static str>::from(&request);
        let (tx, rx) = oneshot::channel();
        self.session.handle.submit(
            request,
            ResultTarget::Caller(tx),
            self.session.config.max_in_flight,
        )?;
        Ok(Pending::new(rx, move |result| decode(result, request_name)))
    }

    /// 提交请求后立即返回，不向调用方交付执行结论或查询载荷。
    ///
    /// 本地未提交当场记日志；daemon 返回 [`OperationResult::Failed`] 时由会话记录。
    /// 需要结论时使用 [`Self::submit`]。
    ///
    /// # Params:
    ///   - `request`: 要交给 daemon 执行的业务请求。
    pub fn fire(&self, request: mineral_protocol::Request) {
        let request_name = <&'static str>::from(&request);
        let target = ResultTarget::LogFailures { request_name };
        if let Err(error) =
            self.session
                .handle
                .submit(request, target, self.session.config.max_in_flight)
        {
            mineral_log::warn!(
                target: "ipc",
                method = request_name,
                error = mineral_log::chain(error),
                "本地未提交"
            );
        }
    }
}

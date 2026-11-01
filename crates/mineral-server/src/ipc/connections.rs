//! 分配连接 ID、统计在线数量；每条连接独占身份与计时数据，结束时结算统计。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use mineral_protocol::ClientInfo;

use crate::client::ClientHandle;

/// 接入任务共享 ID 分配器，连接任务与心跳共享在线计数。
pub(crate) struct ConnectionCounters {
    /// 下一个连接 ID，进程内单调自增、不复用。
    next_id: AtomicU64,

    /// 包括尚未完成握手的连接。
    online: AtomicUsize,
}

impl ConnectionCounters {
    /// 尚无连接的计数器。
    pub(crate) fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            online: AtomicUsize::new(0),
        }
    }

    /// 将新连接计入在线数，返回随连接任务持有的生命周期对象。
    pub(super) fn connect(self: &Arc<Self>, client: &ClientHandle) -> ConnectionLifetime {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        ConnectionLifetime {
            id,
            counters: Arc::clone(self),
            client: client.for_connection(id),
            identity: None,
            concurrent_at_connect: self.online.fetch_add(1, Ordering::Relaxed) + 1,
            connected_at: Instant::now(),
        }
    }

    /// 当前在线连接数(心跳上报用)。
    pub(crate) fn online(&self) -> usize {
        self.online.load(Ordering::Relaxed)
    }
}

/// 随连接任务销毁时减少在线数；只有完成握手的连接才记录时长与建立时的并发数。
pub(super) struct ConnectionLifetime {
    /// 本连接 ID。
    pub(super) id: u64,

    /// 握手完成后填写；未握手或握手被拒绝时为 `None`。
    pub(super) identity: Option<ClientInfo>,

    counters: Arc<ConnectionCounters>,

    client: ClientHandle,

    connected_at: Instant,

    /// 建立时的在线连接数，包含自己。
    concurrent_at_connect: usize,
}

impl Drop for ConnectionLifetime {
    fn drop(&mut self) {
        self.counters.online.fetch_sub(1, Ordering::Relaxed);
        let Some(identity) = self.identity.take() else {
            return;
        };
        let duration_ms =
            i64::try_from(self.connected_at.elapsed().as_millis()).unwrap_or(i64::MAX);
        let concurrent = i64::try_from(self.concurrent_at_connect).unwrap_or(i64::MAX);
        self.client
            .record_client_connection(identity.name, duration_ms, concurrent);
    }
}

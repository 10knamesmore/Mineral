//! 登记在线连接与握手身份，在连接结束时结算连接时长和并发数。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::client::ClientHandle;

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
    pub(super) fn register(&self) -> u64 {
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
    pub(super) fn set_identity(&self, id: u64, info: mineral_protocol::ClientInfo) {
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
pub(super) struct ConnGuard {
    /// 所属注册表。
    pub(super) registry: Arc<ConnRegistry>,

    /// 本连接 id。
    pub(super) id: u64,

    /// 埋点出口(per-conn handle)。
    pub(super) client: ClientHandle,
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

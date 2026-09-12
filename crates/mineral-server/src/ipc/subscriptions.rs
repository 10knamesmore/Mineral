//! 按连接登记订阅任务，处理订阅、退订、重同步与 PCM 游标归还。

use std::sync::Arc;

use mineral_protocol::{SessionMessage, SubscribeRequest, SubscriptionId, SubscriptionTopic};
use rustc_hash::FxHashMap;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use super::session::SessionServices;
use super::updates::{
    pump_downloads_detail, pump_downloads_summary, pump_events, pump_pcm, pump_playback,
    pump_player, pump_tasks,
};
use crate::client::ClientHandle;
use crate::pcm_relay::{PcmRelay, PcmSubscription};

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
pub(super) struct Subscriptions {
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
    pub(super) fn subscribe(
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
    pub(super) fn unsubscribe(&mut self, id: SubscriptionId, pcm: &PcmRelay) {
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
    pub(super) fn resync(&self, id: SubscriptionId) {
        if let Some(pump) = self.pumps.get(&id) {
            pump.resync.notify_one();
        }
    }

    /// 连接收尾:停掉全部泵并归还 PCM 游标。
    ///
    /// # Params:
    ///   - `pcm`: PCM 中继
    pub(super) fn shutdown(&mut self, pcm: &PcmRelay) {
        for (_id, pump) in self.pumps.drain() {
            pump.handle.abort();
            if let Some(token) = pump.pcm {
                pcm.unsubscribe(token);
            }
        }
    }
}

//! 按订阅转发领域状态与事件，维护版本、分片和重同步快照。
//!
//! 每个订阅消费共享发布器的 watch 或 broadcast，按自己的版本发送增量。
//! 事件积压后补发可重放快照；下载明细积压后重新发送完整列表。

use std::sync::Arc;

use mineral_protocol::{
    DownloadDetailDelta, PlayerVersions, SessionMessage, SubscriptionId, UpdateEnvelope,
    UpdatePayload, fragment_download_detail, fragment_player_update,
};
use tokio::sync::{Notify, broadcast, mpsc, watch};

use crate::client::ClientHandle;

/// 播放订阅泵:订阅即发一帧(版本门控),之后按领域变更推增量。
pub(super) async fn pump_player(
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
pub(super) async fn pump_playback(
    rx: watch::Receiver<Arc<mineral_audio::AudioSnapshot>>,
    id: SubscriptionId,
    out: mpsc::Sender<SessionMessage>,
) {
    pump_watch(rx, id, out, |snapshot| {
        UpdatePayload::Playback(Box::new(snapshot.as_ref().clone()))
    })
    .await;
}

/// 任务摘要泵。
pub(super) async fn pump_tasks(
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
pub(super) async fn pump_downloads_summary(
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
pub(super) async fn pump_downloads_detail(
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
pub(super) async fn pump_pcm(
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
pub(super) async fn pump_events(
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

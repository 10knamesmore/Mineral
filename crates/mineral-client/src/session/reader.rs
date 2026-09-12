//! 接收会话消息、交付结果并将订阅更新应用到镜像。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mineral_protocol::{SessionMessage, SubscriptionId, UpdateEnvelope, WireSource};
use tokio::sync::mpsc;

use super::SessionShared;
use super::assembly::Assembler;
use crate::state::ApplyOutcome;

/// reader:处理结果 / 更新 / 关闭,并驱动分片组装与镜像应用。
pub(super) async fn reader_loop(
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
                        let reply = shared.inflight.lock().remove(&result.id);
                        let Some(reply) = reply else {
                            mineral_log::warn!(target: "ipc", id = result.id.value(), "无主结果,丢弃");
                            continue;
                        };
                        shared.in_flight.fetch_sub(1, Ordering::AcqRel);
                        reply.deliver(result.id, result.result);
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

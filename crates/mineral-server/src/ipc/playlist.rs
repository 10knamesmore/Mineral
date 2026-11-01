//! Playlist subscription ownership and cumulative-snapshot delivery.

use futures_util::StreamExt;
use mineral_channel_core::Error;
use mineral_common::revision::Revision;
use mineral_model::PlaylistId;
use mineral_protocol::{
    AdvanceSubscription, Event, SessionMessage, SubscriptionDemand, SubscriptionId, UpdateEnvelope,
    UpdatePayload,
};
use mineral_task::{ChannelFetchKind, TaskEvent};
use tokio::sync::mpsc;

use crate::client::ClientHandle;

/// The subscription owns the stream; each demand uses the existing source queue.
/// Cached snapshots do not satisfy demand. Complete consumption stops only at a complete Fresh.
pub(super) async fn pump_playlist(
    client: ClientHandle,
    subscription: SubscriptionId,
    id: PlaylistId,
    mut demands: mpsc::UnboundedReceiver<AdvanceSubscription>,
    out: mpsc::Sender<SessionMessage>,
) {
    let scheduler = client.scheduler();
    let mut stream = scheduler.playlist_stream(id.clone());
    let mut version = Revision::default();
    while let Some(request) = demands.recv().await {
        loop {
            let result = scheduler
                .run_channel(
                    ChannelFetchKind::PlaylistDetail { id: id.clone() },
                    request.priority,
                    async {
                        stream.next().await.ok_or(Error::InvalidData {
                            field: "playlist stream ended before complete",
                        })?
                    },
                )
                .await;
            let (event, fresh, complete, failed) = match result {
                Ok(snapshot) => {
                    let fresh = snapshot.is_fresh();
                    let detail = snapshot.into_value();
                    let complete = fresh && detail.complete;
                    let event = TaskEvent::PlaylistDetailFetched {
                        id: id.clone(),
                        fresh,
                        detail: Box::new(detail),
                    };
                    (event, fresh, complete, false)
                }
                Err(error) => {
                    mineral_log::warn!(target: "ipc", playlist = %id, error = mineral_log::chain(&error), "playlist stream failed before completion");
                    (
                        TaskEvent::PlaylistDetailFailed { id: id.clone() },
                        false,
                        false,
                        true,
                    )
                }
            };
            version = version.next();
            if out
                .send(SessionMessage::Update(UpdateEnvelope {
                    subscription,
                    version,
                    parts: 1,
                    index: 0,
                    payload: UpdatePayload::Event(Box::new(Event::Task(Box::new(event)))),
                }))
                .await
                .is_err()
            {
                return;
            }
            if complete || failed {
                return;
            }
            if fresh && request.demand == SubscriptionDemand::Next {
                break;
            }
        }
    }
}

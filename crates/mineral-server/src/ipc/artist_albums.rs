//! 艺人专辑订阅按需求推进来源流，失败后保留进度等待重试。

use futures_util::StreamExt;
use mineral_channel_core::Error;
use mineral_common::revision::Revision;
use mineral_model::ArtistId;
use mineral_protocol::{
    AdvanceSubscription, Event, SessionMessage, SubscriptionDemand, SubscriptionId, UpdateEnvelope,
    UpdatePayload,
};
use mineral_task::{ChannelFetchKind, TaskEvent};
use tokio::sync::mpsc;

use crate::client::ClientHandle;

pub(super) async fn pump_artist_albums(
    client: ClientHandle,
    subscription: SubscriptionId,
    id: ArtistId,
    mut demands: mpsc::UnboundedReceiver<AdvanceSubscription>,
    out: mpsc::Sender<SessionMessage>,
) {
    let scheduler = client.scheduler();
    let mut stream = scheduler.artist_albums_stream(id.clone());
    let mut version = Revision::default();
    let mut batch = 0;
    while let Some(request) = demands.recv().await {
        loop {
            let result = scheduler
                .run_channel(
                    ChannelFetchKind::ArtistAlbums { id: id.clone() },
                    request.priority,
                    async {
                        stream.next().await.ok_or(Error::InvalidData {
                            field: "artist albums stream ended before complete",
                        })?
                    },
                )
                .await;
            let (event, complete, failed) = match result {
                Ok(result) => {
                    let complete = result.complete;
                    let event = TaskEvent::ArtistAlbumsFetched {
                        id: id.clone(),
                        batch,
                        albums: result.items,
                        complete,
                    };
                    batch += 1;
                    (event, complete, false)
                }
                Err(error) => {
                    mineral_log::warn!(target: "ipc", artist = %id, batch,
                        error = mineral_log::chain(&error), "artist albums batch failed");
                    (
                        TaskEvent::ArtistAlbumsPageFailed {
                            id: id.clone(),
                            batch,
                        },
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
                || complete
            {
                return;
            }
            if failed || request.demand == SubscriptionDemand::Next {
                break;
            }
        }
    }
}

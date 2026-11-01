//! 搜索订阅持有来源流，按需求交付批次，失败后等待显式重试。

use futures_util::StreamExt;
use mineral_channel_core::Error;
use mineral_common::revision::Revision;
use mineral_model::{SearchKind, SourceKind};
use mineral_protocol::{
    AdvanceSubscription, Event, SessionMessage, SubscriptionDemand, SubscriptionId, UpdateEnvelope,
    UpdatePayload,
};
use mineral_task::{ChannelFetchKind, TaskEvent};
use tokio::sync::mpsc;

use crate::client::ClientHandle;

pub(super) async fn pump_search(
    client: ClientHandle,
    subscription: SubscriptionId,
    source: SourceKind,
    kind: SearchKind,
    query: String,
    mut demands: mpsc::UnboundedReceiver<AdvanceSubscription>,
    out: mpsc::Sender<SessionMessage>,
) {
    let scheduler = client.scheduler();
    let mut stream = scheduler.search_stream(source, kind, query.clone());
    let mut version = Revision::default();
    let mut batch = 0;
    while let Some(request) = demands.recv().await {
        loop {
            let result = scheduler
                .run_channel(
                    ChannelFetchKind::Search {
                        source,
                        kind,
                        query: query.clone(),
                    },
                    request.priority,
                    async {
                        stream.next().await.ok_or(Error::InvalidData {
                            field: "search stream ended before complete",
                        })?
                    },
                )
                .await;
            let (event, complete, failed) = match result {
                Ok(result) => {
                    client.record_search_result(source, kind, &query, batch, &result.payload);
                    let complete = result.complete;
                    let event = TaskEvent::SearchResults {
                        source,
                        kind,
                        query: query.clone(),
                        batch,
                        payload: result.payload,
                        complete,
                    };
                    batch += 1;
                    (event, complete, false)
                }
                Err(error) => {
                    mineral_log::warn!(target: "ipc", ?source, ?kind, query, batch,
                        error = mineral_log::chain(&error), "search batch failed");
                    (
                        TaskEvent::SearchPageFailed {
                            source,
                            kind,
                            query: query.clone(),
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

//! 生产任务提交器在真实 socket 背压下的恢复验证。

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::eyre;
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_model::{AlbumId, ArtistId, PlaylistId, SourceKind};
use mineral_protocol::{
    MessageBatch, OperationResult, Request, ServerHello, SessionMessage, SessionResult, SocketWire,
    Wire,
};
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use super::{Backend, BackendBootstrap, ClientBackend, CompletionQueue};

/// 页面各提交一次即可；统一推进在容量恢复后发完任务，不依赖页面重新登记任务。
#[tokio::test]
async fn shared_submitter_delivers_tasks_after_socket_backpressure() -> color_eyre::Result<()> {
    for (outbound_capacity, max_in_flight, initially_submitted) in [(8, 2, 2), (1, 64, 1)] {
        let tasks = vec![
            TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail {
                id: PlaylistId::new(SourceKind::NETEASE, "playlist"),
            }),
            TaskKind::ChannelFetch(ChannelFetchKind::AlbumDetail {
                id: AlbumId::new(SourceKind::NETEASE, "album"),
            }),
            TaskKind::ChannelFetch(ChannelFetchKind::ArtistDetail {
                id: ArtistId::new(SourceKind::NETEASE, "artist"),
            }),
            TaskKind::ChannelFetch(ChannelFetchKind::Search {
                source: SourceKind::NETEASE,
                kind: mineral_model::SearchKind::Song,
                query: "query".to_owned(),
                page: mineral_channel_core::Page::default(),
            }),
        ];
        let (client_socket, server_socket) = tokio::net::UnixStream::pair()?;
        let (release, wait_for_capacity) = tokio::sync::oneshot::channel();
        let expected_count = tasks.len();
        let server = tokio::spawn(async move {
            let mut wire = SocketWire::from_stream(server_socket);
            wire.recv()
                .await?
                .ok_or_else(|| eyre!("missing handshake"))?;
            wire.send(MessageBatch::one(SessionMessage::Welcome(
                ServerHello::accept(),
            )))
            .await?;
            wait_for_capacity.await?;
            let mut received = Vec::new();
            while received.len() < expected_count {
                let batch = wire
                    .recv()
                    .await?
                    .ok_or_else(|| eyre!("connection ended before all tasks arrived"))?;
                let mut results = Vec::new();
                for message in batch.messages {
                    if let SessionMessage::Requests(requests) = message {
                        for request in requests {
                            let Request::SubmitTask(kind, _) = request.request else {
                                return Err(eyre!("unexpected request"));
                            };
                            received.push(kind);
                            results.push(SessionResult {
                                id: request.id,
                                result: OperationResult::Accepted,
                            });
                        }
                    }
                }
                wire.send(MessageBatch::one(SessionMessage::Results(results)))
                    .await?;
            }
            Ok::<_, color_eyre::Report>(received)
        });
        let client = Client::from_wire(
            Box::new(SocketWire::from_stream(client_socket)),
            "task-submit",
            ClientConfig {
                outbound_capacity,
                max_in_flight,
                ..ClientConfig::default()
            },
        )
        .await?;
        let backend = ClientBackend::new(
            Arc::new(client),
            BackendBootstrap::default(),
            CompletionQueue::new(),
        );
        for task in &tasks {
            backend.submit_task(task.clone(), Priority::Background);
        }
        assert_eq!(
            backend.pending_task_count(),
            tasks.len() - initially_submitted
        );
        let selected = tasks.last().ok_or_else(|| eyre!("missing selected task"))?;
        backend.prioritize_task(selected);
        release
            .send(())
            .map_err(|()| eyre!("server stopped before release"))?;
        let received = tokio::time::timeout(Duration::from_secs(5), async {
            while backend.pending_task_count() > 0 {
                backend.flush_task_submissions();
                tokio::task::yield_now().await;
            }
            server.await?
        })
        .await??;
        let mut expected = tasks
            .iter()
            .take(initially_submitted)
            .cloned()
            .collect::<Vec<_>>();
        expected.push(selected.clone());
        expected.extend(
            tasks
                .iter()
                .take(tasks.len() - 1)
                .skip(initially_submitted)
                .cloned(),
        );
        assert_eq!(received, expected);
        assert_eq!(backend.pending_task_count(), 0);
    }
    Ok(())
}

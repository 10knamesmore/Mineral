//! 会话核心的传输契约测试:同一组行为必须在内存承载与 socket 承载上完全一致。
//!
//! 每个场景都跑 `[TestTransport::Memory, TestTransport::Socket]` 两遍;脚本化 server 只使用
//! 公开的 [`Wire`] / [`SessionMessage`] 形状,不依赖 daemon 实现。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use color_eyre::eyre::{WrapErr, eyre};
use mineral_client::Client;
use mineral_client::connection::{ClientConfig, ConnectError};
use mineral_client::operation::{Outcome, SubmitError};
use mineral_protocol::{
    FailureKind, MessageBatch, OperationResult, PlayQueueError, QueueContextWire, RejectReason,
    Request, Response, ServerHello, SessionMessage, SessionResult, SubscribeRequest,
    SubscriptionId, SubscriptionTopic,
};
use mineral_protocol::{Wire, WireError, WireSink, WireSource};
use tokio::time::timeout;

mod common;

use common::server::{accept_handshake, request_ids};
use common::transport::TestTransport;

/// 「已应用」结论的译码(测试用)。
fn applied(result: OperationResult, _request_name: &'static str) -> Outcome<()> {
    match result {
        OperationResult::Applied => Outcome::Applied(()),
        OperationResult::Accepted => Outcome::Accepted(()),
        OperationResult::Query(response) => Outcome::Failed {
            kind: mineral_protocol::FailureKind::Internal,
            detail: format!("意外查询应答 {response:?}"),
        },
        OperationResult::Failed(failure) => Outcome::Failed {
            kind: failure.kind,
            detail: failure.detail,
        },
    }
}

/// 单请求 → 单结果。
#[tokio::test]
async fn request_result_round_trip() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let batch = wire.recv().await?.ok_or_else(|| eyre!("没有收到请求"))?;
            let ids = request_ids(&batch);
            assert_eq!(ids.len(), 1, "{transport:?}: {batch:?}");
            let id = *ids.first().ok_or_else(|| eyre!("请求 id 缺失"))?;
            wire.send(MessageBatch::one(SessionMessage::Results(vec![
                SessionResult {
                    id,
                    result: OperationResult::Applied,
                },
            ])))
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        let outcome = client.submit(Request::Pause, applied)?.outcome().await;
        assert!(
            matches!(outcome, Outcome::Applied(())),
            "{transport:?}: {outcome:?}"
        );
        server.await??;
    }
    Ok(())
}

/// 队列播放成功返回单位值,业务失败保留类别与诊断信息,不能伪装为已应用。
#[tokio::test]
async fn play_queue_normalizes_business_results() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let cases = [
            (Ok(()), None),
            (Err(PlayQueueError::Empty), Some(FailureKind::Invalid)),
            (
                Err(PlayQueueError::CapacityExceeded { len: 3, cap: 2 }),
                Some(FailureKind::Invalid),
            ),
            (
                Err(PlayQueueError::TargetOutOfBounds { target: 2, len: 2 }),
                Some(FailureKind::Invalid),
            ),
            (
                Err(PlayQueueError::Unavailable {
                    message: "播放服务不可用".to_owned(),
                }),
                Some(FailureKind::Unavailable),
            ),
        ];
        for (response, expected_kind) in cases {
            let expected_detail = response.as_ref().err().map(ToString::to_string);
            let (client_wire, server_wire) = transport.pair(/*capacity*/ 64)?;
            let server = tokio::spawn(async move {
                let mut wire = server_wire;
                accept_handshake(&mut wire).await?;
                let batch = wire.recv().await?.ok_or_else(|| eyre!("没有收到请求"))?;
                let ids = request_ids(&batch);
                assert_eq!(ids.len(), 1, "{transport:?}: {batch:?}");
                let id = *ids.first().ok_or_else(|| eyre!("请求 id 缺失"))?;
                wire.send(MessageBatch::one(SessionMessage::Results(vec![
                    SessionResult {
                        id,
                        result: OperationResult::Query(Box::new(Response::PlayQueue(response))),
                    },
                ])))
                .await?;
                Ok::<(), color_eyre::Report>(())
            });
            let client =
                Client::from_wire(client_wire, "queue-result", ClientConfig::default()).await?;
            let outcome = client
                .play_queue(
                    vec![mineral_test::song("queue-result")],
                    /*target*/ 0,
                    QueueContextWire::Manual,
                )?
                .outcome()
                .await;
            assert_eq!(
                outcome.is_success(),
                expected_kind.is_none(),
                "{transport:?}"
            );
            match (expected_kind, outcome) {
                (None, Outcome::Applied(())) => {}
                (Some(expected), Outcome::Failed { kind, detail }) => {
                    assert_eq!(kind, expected, "{transport:?}");
                    assert_eq!(Some(detail), expected_detail, "{transport:?}");
                }
                (expected, actual) => {
                    return Err(eyre!(
                        "{transport:?}: 期望失败类别 {expected:?},实际 {actual:?}"
                    ));
                }
            }
            server.await??;
        }
    }
    Ok(())
}

/// 多请求并发:结果乱序返回也按会话 id 精确配对。
#[tokio::test]
async fn out_of_order_results_match_by_id() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let batch = wire.recv().await?.ok_or_else(|| eyre!("没有收到请求"))?;
            let ids = request_ids(&batch);
            assert_eq!(ids.len(), 3, "{transport:?}: {batch:?}");
            let mut results = Vec::new();
            for (index, id) in ids.iter().enumerate().rev() {
                let pid = u32::try_from(100 + index)?;
                results.push(SessionResult {
                    id: *id,
                    result: OperationResult::Query(Box::new(
                        mineral_protocol::Response::DaemonInfo { pid },
                    )),
                });
            }
            wire.send(MessageBatch::one(SessionMessage::Results(results)))
                .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        let first = client.daemon_info();
        let second = client.daemon_info();
        let third = client.daemon_info();
        let (first, second, third) = tokio::join!(first, second, third);
        for (pid, outcome) in [(100, first), (101, second), (102, third)] {
            match outcome {
                Outcome::Applied(value) => assert_eq!(value, pid, "{transport:?}"),
                other => return Err(eyre!("{transport:?}: 期望 pid {pid},实际 {other:?}")),
            }
        }
        server.await??;
    }
    Ok(())
}

/// 连续提交的请求合并成一条 `Requests`(writer 吸干当前队列,不凑批也不逐条发)。
#[tokio::test]
async fn consecutive_requests_share_one_batch() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let batch = wire.recv().await?.ok_or_else(|| eyre!("没有收到请求"))?;
            let ids = request_ids(&batch);
            assert_eq!(ids.len(), 8, "{transport:?}: {batch:?}");
            let results = ids
                .into_iter()
                .map(|id| SessionResult {
                    id,
                    result: OperationResult::Applied,
                })
                .collect::<Vec<_>>();
            wire.send(MessageBatch::one(SessionMessage::Results(results)))
                .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        let mut pending = Vec::new();
        for _ in 0..8 {
            pending.push(client.submit(Request::Pause, applied)?);
        }
        for handle in pending {
            assert!(matches!(handle.outcome().await, Outcome::Applied(())));
        }
        let metrics = client.metrics();
        assert_eq!(metrics.requests_sent, 8, "{transport:?}");
        assert_eq!(metrics.batches_sent, 1, "{transport:?}");
        server.await??;
    }
    Ok(())
}

/// daemon 主动 Goodbye:在途请求收束为「结果未知」,链路标记断开,新提交被拒。
#[tokio::test]
async fn goodbye_unknowns_pending_and_closes_session() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let _batch = wire.recv().await?.ok_or_else(|| eyre!("没有收到请求"))?;
            wire.send(MessageBatch::one(SessionMessage::Goodbye(
                mineral_protocol::CloseReason::ServerShutdown,
            )))
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        let pending = client.submit(Request::Pause, applied)?;
        let outcome = timeout(Duration::from_secs(5), pending.outcome()).await?;
        assert!(matches!(outcome, Outcome::Unknown { .. }), "{transport:?}");
        timeout(Duration::from_secs(5), async {
            while client.connected() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("会话未在 Goodbye 后收束")?;
        assert_eq!(
            client.submit(Request::Pause, applied).err(),
            Some(SubmitError::Disconnected),
            "{transport:?}"
        );
        server.await??;
    }
    Ok(())
}

/// client 主动关闭会把 `Close` 交给对端(尽力送达,不静默丢弃)。
#[tokio::test]
async fn client_close_is_delivered() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let mut seen_close = false;
            while let Some(batch) = wire.recv().await? {
                if batch
                    .messages
                    .iter()
                    .any(|message| matches!(message, SessionMessage::Close(_)))
                {
                    seen_close = true;
                    break;
                }
            }
            assert!(seen_close, "{transport:?}: 未收到 Close");
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.close();
        server.await??;
    }
    Ok(())
}

/// 版本不匹配的握手拒绝以结构化错误浮出(不重试、不降级)。
#[tokio::test]
async fn handshake_rejection_is_structured() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            let _hello = wire.recv().await?.ok_or_else(|| eyre!("握手期间关闭"))?;
            wire.send(MessageBatch {
                messages: vec![
                    SessionMessage::Welcome(ServerHello::reject(RejectReason::VersionMismatch)),
                    SessionMessage::Goodbye(mineral_protocol::CloseReason::Protocol {
                        detail: "版本不匹配".to_owned(),
                    }),
                ],
            })
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let error = match Client::from_wire(client_wire, "contract", ClientConfig::default()).await
        {
            Ok(_client) => return Err(eyre!("{transport:?}: 拒绝的握手不应成功")),
            Err(error) => error,
        };
        match error {
            ConnectError::Rejected(rejected) => {
                assert_eq!(
                    rejected.reason(),
                    Some(RejectReason::VersionMismatch),
                    "{transport:?}"
                );
            }
            other => return Err(eyre!("{transport:?}: 期望结构化拒绝,实际 {other}")),
        }
        server.await??;
    }
    Ok(())
}

/// 订阅引用计数:同主题重复订阅只发一次 `Subscribe`,退到 0 才发 `Unsubscribe`。
#[tokio::test]
async fn subscription_refcount_reaches_wire_once() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let mut subscribes = Vec::<SubscribeRequest>::new();
            let mut unsubscribes = Vec::<SubscriptionId>::new();
            let mut requests = 0_usize;
            while requests < 2 {
                let Some(batch) = wire.recv().await? else {
                    break;
                };
                for message in batch.messages {
                    match message {
                        SessionMessage::Subscribe(request) => subscribes.push(request),
                        SessionMessage::Unsubscribe(id) => unsubscribes.push(id),
                        SessionMessage::Requests(list) => {
                            requests = requests.saturating_add(list.len());
                            let results = list
                                .into_iter()
                                .map(|request| SessionResult {
                                    id: request.id,
                                    result: OperationResult::Applied,
                                })
                                .collect::<Vec<_>>();
                            wire.send(MessageBatch::one(SessionMessage::Results(results)))
                                .await?;
                        }
                        _ => {}
                    }
                }
            }
            assert_eq!(subscribes.len(), 1, "{transport:?}: 重复订阅应合并");
            assert_eq!(unsubscribes.len(), 1, "{transport:?}: 归零才退订");
            let id = subscribes.first().map(|request| request.id);
            assert_eq!(
                id,
                unsubscribes.first().copied(),
                "{transport:?}: 退订 id 与订阅一致"
            );
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        let topic = SubscriptionTopic::Playback;
        let first = client.subscribe(topic);
        let second = client.subscribe(topic);
        assert_eq!(first, second, "{transport:?}: 同主题返回同一 id");
        client.unsubscribe(topic);
        assert!(matches!(
            client.submit(Request::Pause, applied)?.outcome().await,
            Outcome::Applied(())
        ));
        client.unsubscribe(topic);
        assert!(matches!(
            client.submit(Request::Pause, applied)?.outcome().await,
            Outcome::Applied(())
        ));
        server.await??;
    }
    Ok(())
}

/// 在途上限:慢对端下超额提交立即报 `InFlightLimit`,不无限排队。
#[tokio::test]
async fn in_flight_limit_is_enforced() -> color_eyre::Result<()> {
    let config = ClientConfig {
        outbound_capacity: 8,
        max_in_flight: 1,
        ..ClientConfig::default()
    };
    let client = Client::from_wire(
        Box::new(StallingWire::new(/*stall_after*/ 1)),
        "contract",
        config,
    )
    .await?;
    let _first = client.submit(Request::Pause, applied)?;
    // 让 writer 取走第一条并卡在发送上;在途计数保持 1。
    tokio::task::yield_now().await;
    assert_eq!(
        client.submit(Request::Pause, applied).err(),
        Some(SubmitError::InFlightLimit)
    );
    Ok(())
}

/// 待发队列上限:writer 卡住时后续提交报 `QueueFull`,不阻塞调用线程。
#[tokio::test]
async fn outbound_queue_is_bounded() -> color_eyre::Result<()> {
    let config = ClientConfig {
        outbound_capacity: 1,
        max_in_flight: 64,
        ..ClientConfig::default()
    };
    let client = Client::from_wire(
        Box::new(StallingWire::new(/*stall_after*/ 1)),
        "contract",
        config,
    )
    .await?;
    let _first = client.submit(Request::Pause, applied)?;
    tokio::task::yield_now().await;
    let _queued = client.submit(Request::Pause, applied)?;
    assert_eq!(
        client.submit(Request::Pause, applied).err(),
        Some(SubmitError::QueueFull)
    );
    Ok(())
}

/// 慢对端不阻塞 reader:writer 卡住时订阅更新照常到达并应用。
#[tokio::test]
async fn reader_stays_live_while_writer_blocks() -> color_eyre::Result<()> {
    let (client_wire, server_wire) = TestTransport::Memory.pair(/*capacity*/ 1)?;
    let server = tokio::spawn(async move {
        let mut wire = server_wire;
        accept_handshake(&mut wire).await?;
        // 之后刻意不再读 client → 方向;持续向 client 推播放锚点。
        for volume in 1_u8..=5 {
            let snapshot = mineral_audio::AudioSnapshot {
                playing: true,
                volume_pct: volume,
                ..mineral_audio::AudioSnapshot::default()
            };
            wire.send(MessageBatch::one(SessionMessage::Update(
                mineral_protocol::UpdateEnvelope {
                    subscription: SubscriptionId::new(0),
                    version: u64::from(volume),
                    parts: 1,
                    index: 0,
                    payload: mineral_protocol::UpdatePayload::Playback(Box::new(snapshot)),
                },
            )))
            .await?;
        }
        // 挂住不读也不关,模拟慢对端。
        std::future::pending::<()>().await;
        Ok::<(), color_eyre::Report>(())
    });
    let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
    client.subscribe(SubscriptionTopic::Playback);
    // 多订阅几次把 writer 堵在容量为 1 的发送通道上。
    for _ in 0..4 {
        client.subscribe(SubscriptionTopic::Pcm);
    }
    timeout(Duration::from_secs(5), async {
        loop {
            if client.playback_snapshot().volume_pct == 5 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .wrap_err("writer 阻塞时 reader 应继续应用更新")?;
    server.abort();
    Ok(())
}

/// 测试用承载:前 `stall_after` 次发送正常完成,之后永久挂起;只回一次 Welcome。
struct StallingWire {
    /// 已完成的发送次数。
    sends: Arc<AtomicUsize>,

    /// 第几次发送起开始挂起。
    stall_after: usize,

    /// 握手应答是否已发出。
    welcomed: bool,
}

impl StallingWire {
    /// 造一条会卡住的承载。
    ///
    /// # Params:
    ///   - `stall_after`: 前多少次发送正常返回
    fn new(stall_after: usize) -> Self {
        Self {
            sends: Arc::new(AtomicUsize::new(0)),
            stall_after,
            welcomed: false,
        }
    }
}

#[async_trait]
impl Wire for StallingWire {
    async fn send(&mut self, _batch: MessageBatch) -> Result<(), WireError> {
        if self.sends.fetch_add(1, Ordering::SeqCst) >= self.stall_after {
            return std::future::pending::<Result<(), WireError>>().await;
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<MessageBatch>, WireError> {
        if !self.welcomed {
            self.welcomed = true;
            return Ok(Some(MessageBatch::one(SessionMessage::Welcome(
                ServerHello::accept(),
            ))));
        }
        std::future::pending::<Result<Option<MessageBatch>, WireError>>().await
    }

    async fn close(&mut self) -> Result<(), WireError> {
        Ok(())
    }

    fn split(self: Box<Self>) -> (Box<dyn WireSink>, Box<dyn WireSource>) {
        (
            Box::new(StallingSink {
                sends: Arc::clone(&self.sends),
                stall_after: self.stall_after,
            }),
            Box::new(StallingSource),
        )
    }
}

/// [`StallingWire`] 的发送半。
struct StallingSink {
    /// 已完成的发送次数(与整体共享)。
    sends: Arc<AtomicUsize>,

    /// 第几次发送起开始挂起。
    stall_after: usize,
}

#[async_trait]
impl WireSink for StallingSink {
    async fn send(&mut self, _batch: MessageBatch) -> Result<(), WireError> {
        if self.sends.fetch_add(1, Ordering::SeqCst) >= self.stall_after {
            return std::future::pending::<Result<(), WireError>>().await;
        }
        Ok(())
    }

    async fn close(&mut self) -> Result<(), WireError> {
        Ok(())
    }
}

/// [`StallingWire`] 的接收半:握手后永不产出。
struct StallingSource;

#[async_trait]
impl WireSource for StallingSource {
    async fn recv(&mut self) -> Result<Option<MessageBatch>, WireError> {
        std::future::pending::<Result<Option<MessageBatch>, WireError>>().await
    }
}

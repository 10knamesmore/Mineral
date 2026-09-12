//! 分片组装契约:大队列 / 明细快照按 `(subscription, version)` 原子组装,缺片 / 重复 / 超限
//! 都收敛为显式重同步请求。

use color_eyre::eyre::{WrapErr, eyre};
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
use mineral_client::state::DownloadsDetailMirror;
use mineral_model::Song;
use mineral_protocol::{
    DownloadId, DownloadOrigin, DownloadStatus, MessageBatch, OperationResult, PlayCursor,
    PlayMode, PlayerSync, PlayerVersions, QueueSync, Request, SegmentVersion, SessionMessage,
    SessionResult, SongDownloadView, SubscriptionId, SubscriptionTopic, UpdateEnvelope,
    UpdatePayload, assembly_limits, fragment_download_detail, fragment_player_update,
};
use tokio::time::timeout;

mod common;

use common::server::accept_handshake;
use common::transport::TestTransport;

/// 「已应用」结论的译码(测试用)。
fn applied(result: OperationResult, _request_name: &'static str) -> Outcome<()> {
    match result {
        OperationResult::Applied => Outcome::Applied(()),
        OperationResult::Accepted => Outcome::Accepted(()),
        OperationResult::Failed(failure) => Outcome::Failed {
            kind: failure.kind,
            detail: failure.detail,
        },
        OperationResult::Query(response) => Outcome::Failed {
            kind: mineral_protocol::FailureKind::Internal,
            detail: format!("意外查询应答 {response:?}"),
        },
    }
}

/// 造 `len` 首歌的队列。
fn queue_of(len: usize) -> Vec<Song> {
    (0..len)
        .map(|index| mineral_test::song(&format!("s{index}")))
        .collect()
}

/// 造一份带 queue 重段的播放同步。
fn sync_with_queue(version: u64, queue: Vec<Song>) -> PlayerSync {
    PlayerSync {
        versions: PlayerVersions {
            queue: SegmentVersion::new(version),
            current: SegmentVersion::ZERO,
        },
        cursor: PlayCursor::default(),
        play_mode: PlayMode::default(),
        play_origin: None,
        queue: Some(QueueSync {
            queue,
            original_queue: None,
        }),
        current: None,
    }
}

/// 造一行下载明细。
fn row(index: usize) -> SongDownloadView {
    let song = mineral_test::song(&format!("d{index}"));
    SongDownloadView {
        id: DownloadId::new(format!("d{index}")),
        song: Box::new(song),
        origin: DownloadOrigin::Direct,
        status: DownloadStatus::Downloading,
        quality: mineral_model::BitRate::Exhigh,
        bytes_done: u64::try_from(index).unwrap_or_default(),
        bytes_total: Some(1_000_000),
        speed_bps: 1024,
        failure: None,
    }
}

/// 等订阅的播放队列版本到位(订阅即推,正常毫秒级)。
async fn wait_versions(client: &Client, expected: SegmentVersion) -> color_eyre::Result<()> {
    timeout(std::time::Duration::from_secs(5), async {
        loop {
            let versions = client
                .mirror()
                .read_player(|player| player.versions().queue);
            if versions == expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .wrap_err("播放版本未在期限内到位")
}

/// 300 首队列分两片,片间穿插一条请求结果:镜像只在收齐后整体更替。
#[tokio::test]
async fn player_queue_fragments_assemble_atomically() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let mut subscription = None;
            let mut request = None;
            while subscription.is_none() || request.is_none() {
                let Some(batch) = wire.recv().await? else {
                    break;
                };
                for message in batch.messages {
                    match message {
                        SessionMessage::Subscribe(subscribe) => subscription = Some(subscribe.id),
                        SessionMessage::Requests(list) => {
                            request = list.first().map(|request| request.id);
                        }
                        _ => {}
                    }
                }
            }
            let subscription = subscription.ok_or_else(|| eyre!("没有收到订阅"))?;
            let request = request.ok_or_else(|| eyre!("没有收到请求"))?;
            let parts = fragment_player_update(
                subscription,
                1,
                sync_with_queue(/*version*/ 1, queue_of(300)),
            );
            assert_eq!(parts.len(), 3, "{transport:?}: 300 首应拆成头段 + 2 片");
            wire.send(MessageBatch::one(
                parts.first().cloned().ok_or_else(|| eyre!("分片组为空"))?,
            ))
            .await?;
            // 片间穿插结果帧:控制路径不被大载荷独占。
            wire.send(MessageBatch::one(SessionMessage::Results(vec![
                SessionResult {
                    id: request,
                    result: OperationResult::Applied,
                },
            ])))
            .await?;
            for part in parts.iter().skip(1) {
                wire.send(MessageBatch::one(part.clone())).await?;
            }
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::Player);
        let pending = client.submit(Request::Pause, applied)?;
        assert!(matches!(pending.outcome().await, Outcome::Applied(())));
        wait_versions(&client, SegmentVersion::FIRST).await?;
        let queue = client.mirror().read_player(|player| player.queue().len());
        assert_eq!(queue, 300, "{transport:?}");
        server.await??;
    }
    Ok(())
}

/// 缺片组被更新版本取代 = 组装中断:client 发 `Resync`,补发完整快照后镜像才更新。
#[tokio::test]
async fn missing_part_requests_resync() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            let parts = fragment_player_update(
                subscription,
                1,
                sync_with_queue(/*version*/ 1, queue_of(300)),
            );
            // 只发头段,制造缺片。
            wire.send(MessageBatch::one(
                parts.first().cloned().ok_or_else(|| eyre!("分片组为空"))?,
            ))
            .await?;
            // 下一个版本到来 = 旧组永远不会补齐 → client 必须请求重同步。
            let next = fragment_player_update(
                subscription,
                2,
                sync_with_queue(/*version*/ 2, queue_of(300)),
            );
            wire.send(MessageBatch::one(
                next.first().cloned().ok_or_else(|| eyre!("分片组为空"))?,
            ))
            .await?;
            let resync = wait_resync(&mut wire).await?;
            assert_eq!(resync, subscription, "{transport:?}: Resync 应指向该订阅");
            // 补发整段快照:重同步后版本基线由 1 重新建立(与 daemon 泵一致)。
            wire.send(MessageBatch::one(SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 1,
                parts: 1,
                index: 0,
                payload: UpdatePayload::Player {
                    sync: Box::new(sync_with_queue(/*version*/ 1, queue_of(300))),
                    queue_parts: 0,
                },
            })))
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::Player);
        wait_versions(&client, SegmentVersion::FIRST).await?;
        let queue = client.mirror().read_player(|player| player.queue().len());
        assert_eq!(queue, 300, "{transport:?}");
        server.await??;
    }
    Ok(())
}

/// 重复分片是协议违规:记丢弃并请求重同步,不重复应用。
#[tokio::test]
async fn duplicate_part_is_rejected() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            let parts = fragment_player_update(
                subscription,
                1,
                sync_with_queue(/*version*/ 1, queue_of(300)),
            );
            let head = parts.first().cloned().ok_or_else(|| eyre!("分片组为空"))?;
            wire.send(MessageBatch::one(head.clone())).await?;
            wire.send(MessageBatch::one(head)).await?;
            let _resync = wait_resync(&mut wire).await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::Player);
        timeout(std::time::Duration::from_secs(5), async {
            loop {
                if client.metrics().updates_dropped > 0 {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("重复分片应记入丢弃计数")?;
        server.await??;
    }
    Ok(())
}

/// 分片数超过硬上限:明确失败并请求重同步,不无限缓冲。
#[tokio::test]
async fn too_many_parts_is_rejected() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            wire.send(MessageBatch::one(SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 1,
                parts: assembly_limits::MAX_PARTS.saturating_add(1),
                index: 0,
                payload: UpdatePayload::Pcm(mineral_protocol::PcmChunk {
                    generation: 1,
                    position: 0,
                    gap: false,
                    sample_rate: Some(44_100),
                    samples: vec![0.0],
                }),
            })))
            .await?;
            let _resync = wait_resync(&mut wire).await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::Pcm);
        timeout(std::time::Duration::from_secs(5), async {
            loop {
                if client.metrics().updates_dropped > 0 {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("超限分片应记入丢弃计数")?;
        server.await??;
    }
    Ok(())
}

/// 600 行下载明细分两片:顺序与行内容整体到位。
#[tokio::test]
async fn downloads_detail_fragments_assemble_in_order() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            let rows = (0..600).map(row).collect::<Vec<_>>();
            let parts = fragment_download_detail(subscription, 1, rows);
            assert_eq!(parts.len(), 3, "{transport:?}: 600 行应拆成头段 + 2 片");
            for part in parts {
                wire.send(MessageBatch::one(part)).await?;
            }
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::DownloadsDetail);
        timeout(std::time::Duration::from_secs(5), async {
            loop {
                let len = client
                    .mirror()
                    .read_downloads_detail(|detail| detail.map(DownloadsDetailMirror::len));
                if len == Some(600) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("下载明细未组装完成")?;
        let ids = client
            .mirror()
            .read_downloads_detail(|detail| {
                detail.map(|detail| {
                    detail
                        .rows()
                        .iter()
                        .map(|row| row.id.as_str().to_owned())
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        assert_eq!(ids.first().map(String::as_str), Some("d0"), "{transport:?}");
        assert_eq!(
            ids.get(599).map(String::as_str),
            Some("d599"),
            "{transport:?}"
        );
        server.await??;
    }
    Ok(())
}

/// 等一条 `Subscribe`,返回其订阅 id。
///
/// # Params:
///   - `wire`: server 侧承载
async fn read_subscription(
    wire: &mut Box<dyn mineral_protocol::Wire>,
) -> color_eyre::Result<SubscriptionId> {
    while let Some(batch) = wire.recv().await? {
        for message in batch.messages {
            if let SessionMessage::Subscribe(subscribe) = message {
                return Ok(subscribe.id);
            }
        }
    }
    Err(eyre!("没有收到订阅"))
}

/// 等 client 发来 `Resync`。
///
/// # Params:
///   - `wire`: server 侧承载
async fn wait_resync(
    wire: &mut Box<dyn mineral_protocol::Wire>,
) -> color_eyre::Result<SubscriptionId> {
    timeout(std::time::Duration::from_secs(5), async {
        loop {
            match wire.recv().await? {
                Some(batch) => {
                    for message in batch.messages {
                        if let SessionMessage::Resync(id) = message {
                            return Ok(id);
                        }
                    }
                }
                None => return Err(eyre!("对端关闭")),
            }
        }
    })
    .await
    .wrap_err("client 未在期限内请求重同步")?
}

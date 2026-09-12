//! 状态订阅在 client 镜像上的语义:版本门控、增量缺基准重同步、本地钟推进位置,
//! 以及下载明细的静态 / 进度分离与顺序变更。

use std::time::Duration;

use color_eyre::eyre::{WrapErr, eyre};
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
use mineral_client::state::DownloadsDetailMirror;
use mineral_protocol::{
    DownloadDetailDelta, DownloadDetailUpdate, DownloadId, DownloadOrigin, DownloadStatus,
    DownloadSummary, MessageBatch, Request, SegmentVersion, SessionMessage, SessionResult,
    SongDownloadView, SubscriptionId, SubscriptionTopic, UpdateEnvelope, UpdatePayload,
};
use tokio::time::timeout;

mod common;

use common::server::{accept_handshake, request_ids};
use common::transport::TestTransport;

/// 「已应用」结论的译码(测试用)。
fn applied(result: mineral_protocol::OperationResult, _request_name: &'static str) -> Outcome<()> {
    match result {
        mineral_protocol::OperationResult::Applied => Outcome::Applied(()),
        mineral_protocol::OperationResult::Failed(failure) => Outcome::Failed {
            kind: failure.kind,
            detail: failure.detail,
        },
        other => Outcome::Failed {
            kind: mineral_protocol::FailureKind::Internal,
            detail: format!("意外结论 {other:?}"),
        },
    }
}

/// 造一行下载明细。
fn row(id: &str, status: DownloadStatus, bytes: u64) -> SongDownloadView {
    SongDownloadView {
        id: DownloadId::new(id.to_owned()),
        song: Box::new(mineral_test::song(id)),
        origin: DownloadOrigin::Direct,
        status,
        quality: mineral_model::BitRate::Exhigh,
        bytes_done: bytes,
        bytes_total: Some(10_000),
        speed_bps: 512,
        failure: None,
    }
}

/// 等一条 `Subscribe` 并返回其 id。
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
async fn wait_resync(
    wire: &mut Box<dyn mineral_protocol::Wire>,
) -> color_eyre::Result<SubscriptionId> {
    timeout(Duration::from_secs(5), async {
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

/// 把一条更新发成单帧。
async fn send_update(
    wire: &mut Box<dyn mineral_protocol::Wire>,
    subscription: SubscriptionId,
    version: u64,
    payload: UpdatePayload,
) -> color_eyre::Result<()> {
    wire.send(MessageBatch::one(SessionMessage::Update(UpdateEnvelope {
        subscription,
        version,
        parts: 1,
        index: 0,
        payload,
    })))
    .await?;
    Ok(())
}

/// 版本缺口触发重同步并重置基线;重复版本被忽略;轻段更新保持重段不变。
#[tokio::test]
async fn player_updates_are_version_gated() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            // 版本 3 越过基线 → client 请求重同步并清空基线。
            send_update(
                &mut wire,
                subscription,
                3,
                UpdatePayload::Player {
                    sync: Box::new(light_sync(/*cursor*/ 1, /*queue_version*/ 0)),
                    queue_parts: 0,
                },
            )
            .await?;
            let resync = wait_resync(&mut wire).await?;
            assert_eq!(resync, subscription, "{transport:?}");
            // 重同步后 daemon 从版本 1 重发完整快照。
            send_update(
                &mut wire,
                subscription,
                1,
                UpdatePayload::Player {
                    sync: Box::new(full_sync(/*cursor*/ 0, /*queue*/ 2)),
                    queue_parts: 0,
                },
            )
            .await?;
            // 重复版本被忽略。
            send_update(
                &mut wire,
                subscription,
                1,
                UpdatePayload::Player {
                    sync: Box::new(full_sync(/*cursor*/ 0, /*queue*/ 5)),
                    queue_parts: 0,
                },
            )
            .await?;
            // 连续版本只带轻段:队列保持。
            send_update(
                &mut wire,
                subscription,
                2,
                UpdatePayload::Player {
                    sync: Box::new(light_sync(/*cursor*/ 1, /*queue_version*/ 1)),
                    queue_parts: 0,
                },
            )
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::Player);
        // 等版本 2 落地:队列仍应是版本 1 的 2 首(重复版本 5 首被忽略)。
        timeout(Duration::from_secs(5), async {
            loop {
                let ready = client.mirror().read_player(|player| {
                    player.cursor().anchor() == 1 && player.queue().len() == 2
                });
                if ready {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("版本 2 未在期限内应用")?;
        let (queue_len, cursor) = client
            .mirror()
            .read_player(|player| (player.queue().len(), player.cursor().anchor()));
        assert_eq!(queue_len, 2, "{transport:?}: 重复版本不应覆盖队列");
        assert_eq!(cursor, 1, "{transport:?}");
        server.await??;
    }
    Ok(())
}

/// 播放锚点用本地单调钟推进,并按已知时长钳位。
#[tokio::test]
async fn playback_anchor_advances_locally() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            send_update(
                &mut wire,
                subscription,
                1,
                UpdatePayload::Playback(Box::new(mineral_audio::AudioSnapshot {
                    playing: true,
                    position_ms: 1_000,
                    duration_ms: Some(1_010),
                    ..mineral_audio::AudioSnapshot::default()
                })),
            )
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::Playback);
        timeout(Duration::from_secs(5), async {
            loop {
                if client
                    .mirror()
                    .read_playback(|playback| playback.anchor().playing)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("锚点未在期限内到达")?;
        tokio::time::sleep(Duration::from_millis(40)).await;
        let position = client.playback_position_ms();
        assert!(position >= 1_000, "{transport:?}: 位置应向前推进");
        assert!(position <= 1_010, "{transport:?}: 位置应钳在时长内");
        server.await??;
    }
    Ok(())
}

/// 摘要更新写入镜像;配置进入事件队列,窗口标题同时更新覆盖状态。
#[tokio::test]
async fn summary_and_event_updates_reach_mirror() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            let tasks = mineral_task::Snapshot {
                running: 3,
                by_kind: rustc_hash::FxHashMap::default(),
            };
            send_update(
                &mut wire,
                subscription,
                1,
                UpdatePayload::Tasks(Box::new(tasks)),
            )
            .await?;
            send_update(
                &mut wire,
                subscription,
                2,
                UpdatePayload::DownloadsSummary(DownloadSummary {
                    active: 2,
                    queued: 1,
                    preparing_playlists: 0,
                    speed_bps: 2048,
                    latest_wave: None,
                }),
            )
            .await?;
            send_update(
                &mut wire,
                subscription,
                3,
                UpdatePayload::Event(Box::new(mineral_protocol::Event::ConfigChanged {
                    config: mineral_protocol::BusValue::Nil,
                })),
            )
            .await?;
            send_update(
                &mut wire,
                subscription,
                4,
                UpdatePayload::Event(Box::new(mineral_protocol::Event::WindowTitleOverride {
                    text: Some("title".to_owned()),
                })),
            )
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::Tasks);
        // 配置由事件队列交给消费方,镜像不保存配置树。
        let mut config_seen = false;
        timeout(Duration::from_secs(5), async {
            loop {
                for event in client.mirror().drain_events() {
                    match event {
                        mineral_protocol::Event::ConfigChanged { .. } => config_seen = true,
                        mineral_protocol::Event::Toast { .. }
                        | mineral_protocol::Event::Card { .. }
                        | mineral_protocol::Event::PropertyChanged { .. }
                        | mineral_protocol::Event::TrackFinished { .. }
                        | mineral_protocol::Event::DownloadCompleted { .. }
                        | mineral_protocol::Event::StoreChanged { .. }
                        | mineral_protocol::Event::ScriptReloaded
                        | mineral_protocol::Event::BusMessage { .. }
                        | mineral_protocol::Event::WindowTitleOverride { .. }
                        | mineral_protocol::Event::DismissToast { .. }
                        | mineral_protocol::Event::Task(_) => {}
                    }
                }
                let ready = client
                    .tasks_snapshot()
                    .is_some_and(|snapshot| snapshot.running == 3)
                    && client
                        .mirror()
                        .read_downloads_summary(|summary| summary.active == 2)
                    && config_seen
                    && matches!(
                        client.mirror().window_title_override(),
                        mineral_client::state::WindowTitleOverride::Set(Some(text)) if text == "title"
                    );
                if ready {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("摘要 / 事件未在期限内应用")?;
        server.await??;
    }
    Ok(())
}

/// 下载明细:快照 → 顺序变更 → 进度 / 新增 / 移除增量按序应用。
#[tokio::test]
async fn downloads_detail_snapshot_and_delta() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            send_update(
                &mut wire,
                subscription,
                1,
                UpdatePayload::DownloadsDetailSnapshot(vec![
                    row("a", DownloadStatus::Downloading, 1),
                    row("b", DownloadStatus::Queued, 0),
                    row("c", DownloadStatus::Downloaded, 10_000),
                ]),
            )
            .await?;
            send_update(
                &mut wire,
                subscription,
                2,
                UpdatePayload::DownloadsDetailDelta(DownloadDetailDelta {
                    order: Some(vec![
                        DownloadId::new("c".to_owned()),
                        DownloadId::new("a".to_owned()),
                    ]),
                    changes: vec![
                        DownloadDetailUpdate::Progress {
                            id: DownloadId::new("a".to_owned()),
                            status: DownloadStatus::Downloading,
                            bytes_done: 5_000,
                            bytes_total: Some(10_000),
                            speed_bps: 1_024,
                            failure: None,
                        },
                        DownloadDetailUpdate::Upsert(Box::new(row("d", DownloadStatus::Queued, 0))),
                        DownloadDetailUpdate::Remove(DownloadId::new("b".to_owned())),
                    ],
                }),
            )
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::DownloadsDetail);
        timeout(Duration::from_secs(5), async {
            loop {
                let ready = client.mirror().read_downloads_detail(|detail| {
                    detail.is_some_and(|detail| {
                        detail.len() == 3
                            && detail
                                .rows()
                                .first()
                                .is_some_and(|row| row.id.as_str() == "c")
                    })
                });
                if ready {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("明细增量未在期限内应用")?;
        let rows = client
            .mirror()
            .read_downloads_detail(|detail| detail.map(DownloadsDetailMirror::rows))
            .unwrap_or_default();
        let ids = rows
            .iter()
            .map(|row| row.id.as_str().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["c", "a", "d"], "{transport:?}");
        let a = rows.iter().find(|row| row.id.as_str() == "a");
        assert!(
            a.is_some_and(|row| row.bytes_done == 5_000 && row.speed_bps == 1_024),
            "{transport:?}: 进度增量应只改动态字段"
        );
        server.await??;
    }
    Ok(())
}

/// 退订后旧 id 的迟到更新不得污染镜像。
#[tokio::test]
async fn late_updates_after_unsubscribe_are_ignored() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let (client_wire, server_wire) = transport.pair(64)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let subscription = read_subscription(&mut wire).await?;
            send_update(
                &mut wire,
                subscription,
                1,
                UpdatePayload::DownloadsDetailSnapshot(vec![row(
                    "a",
                    DownloadStatus::Downloading,
                    1,
                )]),
            )
            .await?;
            // 等 client 发一条请求,确认它已处理完快照并完成退订。
            let batch = wire.recv().await?.ok_or_else(|| eyre!("没有收到请求"))?;
            let ids = request_ids(&batch);
            let id = *ids.first().ok_or_else(|| eyre!("请求 id 缺失"))?;
            wire.send(MessageBatch::one(SessionMessage::Results(vec![
                SessionResult {
                    id,
                    result: mineral_protocol::OperationResult::Applied,
                },
            ])))
            .await?;
            send_update(
                &mut wire,
                subscription,
                2,
                UpdatePayload::DownloadsDetailSnapshot(vec![row(
                    "zombie",
                    DownloadStatus::Downloading,
                    1,
                )]),
            )
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let client = Client::from_wire(client_wire, "contract", ClientConfig::default()).await?;
        client.subscribe(SubscriptionTopic::DownloadsDetail);
        timeout(Duration::from_secs(5), async {
            loop {
                if client
                    .mirror()
                    .read_downloads_detail(|detail| detail.is_some_and(|detail| detail.len() == 1))
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .wrap_err("首个快照未在期限内应用")?;
        client.unsubscribe(SubscriptionTopic::DownloadsDetail);
        // 退订让明细槽位清空;之后到达的旧 id 更新必须被忽略。
        assert!(matches!(
            client.submit(Request::Pause, applied)?.outcome().await,
            Outcome::Applied(())
        ));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            client
                .mirror()
                .read_downloads_detail(|detail| detail.is_none()),
            "{transport:?}: 退订后不应出现旧 id 的更新"
        );
        server.await??;
    }
    Ok(())
}

/// 造一条只带轻段的播放同步(`queue_version` 是 daemon 当前队列版本,轻段要如实带)。
fn light_sync(cursor: usize, queue_version: u64) -> mineral_protocol::PlayerSync {
    mineral_protocol::PlayerSync {
        versions: mineral_protocol::PlayerVersions {
            queue: SegmentVersion::new(queue_version),
            current: SegmentVersion::ZERO,
        },
        cursor: mineral_protocol::PlayCursor::InQueue(cursor),
        ..mineral_protocol::PlayerSync::default()
    }
}

/// 造一条带队列重段的播放同步。
fn full_sync(cursor: usize, queue: usize) -> mineral_protocol::PlayerSync {
    mineral_protocol::PlayerSync {
        versions: mineral_protocol::PlayerVersions {
            queue: SegmentVersion::FIRST,
            current: SegmentVersion::ZERO,
        },
        cursor: mineral_protocol::PlayCursor::InQueue(cursor),
        queue: Some(mineral_protocol::QueueSync {
            queue: (0..queue)
                .map(|index| mineral_test::song(&format!("q{index}")))
                .collect(),
            original_queue: None,
        }),
        ..mineral_protocol::PlayerSync::default()
    }
}

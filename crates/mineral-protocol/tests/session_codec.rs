//! 会话消息的双 codec 守卫:wire 类型只依赖 serde derive,JSON 与 bincode 都能保真
//! 往返(codec 可换,不绑死 bincode)。

use color_eyre::eyre::eyre;

use mineral_model::SongId;
use mineral_protocol::{
    BusValue, ClientInfo, CloseReason, DownloadDetailDelta, DownloadDetailUpdate, DownloadId,
    DownloadOrigin, DownloadStatus, DownloadSummary, DownloadWave, FailureKind, MessageBatch,
    OperationFailure, OperationResult, PcmChunk, PlayerSync, Request, RequestId, Response,
    ServerHello, SessionMessage, SessionRequest, SessionResult, SongDownloadView, SubscribeRequest,
    SubscriptionId, SubscriptionTopic, UpdateEnvelope, UpdatePayload, decode, encode, framed, recv,
    send,
};
use tokio::io::duplex;

/// 同一值经 serde_json 往返,断言 Debug 保真。
fn json_round_trips<T>(value: &T) -> color_eyre::Result<()>
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let want = format!("{value:?}");
    let json = serde_json::to_string(value)?;
    let back: T = serde_json::from_str(&json)?;
    assert_eq!(format!("{back:?}"), want, "JSON 往返应保真");
    Ok(())
}

/// 走 framed bincode 往返(与 [`SocketWire`](mineral_protocol::SocketWire) 同编码)。
async fn framed_round_trips<T>(value: T) -> color_eyre::Result<()>
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let (a, b) = duplex(256 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);
    let want = format!("{value:?}");
    send(&mut sender, &value).await?;
    let got: T = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    assert_eq!(format!("{got:?}"), want);
    Ok(())
}

/// 造一行下载明细。
fn download_row() -> SongDownloadView {
    SongDownloadView {
        id: DownloadId::new("dl-1".to_owned()),
        song: Box::new(mineral_test::song("dl")),
        origin: DownloadOrigin::Direct,
        status: DownloadStatus::Downloading,
        quality: mineral_model::BitRate::Lossless,
        bytes_done: 1_024,
        bytes_total: Some(4_096),
        speed_bps: 512,
        failure: None,
    }
}

/// 一条批里放全部会话消息形状:bincode 与 JSON 都保真。
#[tokio::test]
async fn session_batch_round_trips() -> color_eyre::Result<()> {
    let subscription = SubscriptionId::new(7);
    let batch = MessageBatch {
        messages: vec![
            SessionMessage::Hello(ClientInfo::new("codec")),
            SessionMessage::Welcome(ServerHello::accept()),
            SessionMessage::Requests(vec![SessionRequest {
                id: RequestId::new(3),
                request: Request::DaemonInfo,
            }]),
            SessionMessage::Results(vec![SessionResult {
                id: RequestId::new(3),
                result: OperationResult::Query(Box::new(Response::DaemonInfo { pid: 42 })),
            }]),
            SessionMessage::Subscribe(SubscribeRequest {
                id: subscription,
                topic: SubscriptionTopic::Player,
                known_player: None,
            }),
            SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 9,
                parts: 2,
                index: 1,
                payload: UpdatePayload::Player {
                    sync: Box::new(PlayerSync::default()),
                    queue_parts: 1,
                },
            }),
            SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 10,
                parts: 1,
                index: 0,
                payload: UpdatePayload::Playback(Box::default()),
            }),
            SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 11,
                parts: 1,
                index: 0,
                payload: UpdatePayload::Pcm(PcmChunk {
                    generation: 2,
                    position: 4_096,
                    gap: true,
                    sample_rate: Some(44_100),
                    samples: vec![0.0, 0.25, -0.25],
                }),
            }),
            SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 12,
                parts: 1,
                index: 0,
                payload: UpdatePayload::DownloadsSummary(DownloadSummary {
                    active: 1,
                    queued: 2,
                    preparing_playlists: 0,
                    speed_bps: 1_024,
                    latest_wave: Some(DownloadWave {
                        sequence: 1,
                        downloaded: 2,
                        already_present: 0,
                        skipped_by_hook: 0,
                        failed: 0,
                        stopped: 0,
                    }),
                }),
            }),
            SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 13,
                parts: 1,
                index: 0,
                payload: UpdatePayload::DownloadsDetailDelta(DownloadDetailDelta {
                    order: Some(vec![DownloadId::new("dl-1".to_owned())]),
                    changes: vec![DownloadDetailUpdate::Upsert(Box::new(download_row()))],
                }),
            }),
            SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 14,
                parts: 1,
                index: 0,
                payload: UpdatePayload::Event(Box::new(mineral_protocol::Event::ConfigChanged {
                    config: BusValue::Nil,
                })),
            }),
            SessionMessage::Unsubscribe(subscription),
            SessionMessage::Resync(subscription),
            SessionMessage::Close(CloseReason::ClientClosed),
            SessionMessage::Goodbye(CloseReason::Protocol {
                detail: "版本不匹配".to_owned(),
            }),
        ],
    };
    json_round_trips(&batch)?;
    framed_round_trips(batch.clone()).await?;
    // length-delimited 字节编码直接过一遍(与 SocketWire 同路径)。
    let bytes = encode(&batch)?;
    let back: MessageBatch = decode(&bytes)?;
    assert_eq!(format!("{back:?}"), format!("{batch:?}"));
    Ok(())
}

/// 任务回包随会话更新传输，保留来源、实体身份和载荷。
#[tokio::test]
async fn task_events_round_trip() -> color_eyre::Result<()> {
    use mineral_model::{PlaylistId, SourceKind};
    use mineral_task::TaskEvent;

    let cases = [
        TaskEvent::PlaylistsFetched {
            source: SourceKind::NETEASE,
            playlists: vec![],
        },
        TaskEvent::LikedSongIdsFetched {
            source: SourceKind::NETEASE,
            ids: [SongId::new(SourceKind::NETEASE, "liked")]
                .into_iter()
                .collect(),
        },
        TaskEvent::LocalPlayCountFetched {
            song_id: SongId::new(SourceKind::NETEASE, "s"),
            count: Some(7),
        },
        TaskEvent::PlaylistDetailFetched {
            id: PlaylistId::new(SourceKind::NETEASE, "p"),
            load: mineral_channel_core::PlaylistLoad::Complete,
            detail: Box::new(mineral_channel_core::PlaylistDetail::complete(
                mineral_model::Playlist::builder()
                    .id(PlaylistId::new(SourceKind::NETEASE, "p"))
                    .name("fixture".to_owned())
                    .build(),
            )),
        },
    ];
    for event in cases {
        let message = SessionMessage::Update(UpdateEnvelope {
            subscription: SubscriptionId::new(7),
            version: 1,
            parts: 1,
            index: 0,
            payload: UpdatePayload::Event(Box::new(mineral_protocol::Event::Task(Box::new(event)))),
        });
        json_round_trips(&message)?;
        framed_round_trips(message).await?;
    }
    Ok(())
}

/// 失败结论与查询载荷在两种编码下都不丢字段。
#[tokio::test]
async fn operation_results_round_trip() -> color_eyre::Result<()> {
    for result in [
        OperationResult::Applied,
        OperationResult::Accepted,
        OperationResult::Failed(OperationFailure {
            kind: FailureKind::Unavailable,
            detail: "脚本未启用".to_owned(),
        }),
        OperationResult::Query(Box::new(Response::SongStats(None))),
        OperationResult::Query(Box::new(Response::StoreValue(
            mineral_protocol::StoreValue::Int(-7),
        ))),
        OperationResult::Query(Box::new(Response::SongStats(Some(
            mineral_protocol::SongStatsWire {
                play_count: 2,
                skip_count: 1,
                total_listen_ms: 90_000,
                last_played_at: None,
                loved: true,
            },
        )))),
    ] {
        json_round_trips(&result)?;
        framed_round_trips(result).await?;
    }
    Ok(())
}

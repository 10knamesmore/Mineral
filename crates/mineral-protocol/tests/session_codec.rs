//! 会话消息的双 codec 守卫:wire 类型只依赖 serde derive,JSON 与 bincode 都能保真
//! 往返(codec 可换,不绑死 bincode)。

use color_eyre::eyre::eyre;

use mineral_model::SongId;
use mineral_protocol::{
    BusValue, ClientInfo, CloseReason, CodecError, DownloadDetailDelta, DownloadDetailUpdate,
    DownloadId, DownloadOrigin, DownloadStatus, DownloadSummary, DownloadWave, FailureKind,
    FailureNotice, MessageBatch, OperationFailure, OperationResult, PcmChunk, PlayerSync, Request,
    RequestId, Response, ServerHello, SessionMessage, SessionRequest, SessionResult, SocketWire,
    SongDownloadView, SubscribeRequest, Subscription, SubscriptionId, SubscriptionTopic,
    UpdateEnvelope, UpdatePayload, Wire, WireError, decode, encode, framed, recv, send,
};
use tokio::io::{AsyncWriteExt, duplex};

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
            SessionMessage::Update(UpdateEnvelope {
                subscription,
                version: 15,
                parts: 1,
                index: 0,
                payload: UpdatePayload::Event(Box::new(mineral_protocol::Event::Failure(
                    FailureNotice::ConfigRejected {
                        fields: vec!["tui.behavior.volume_step".to_owned()],
                    },
                ))),
            }),
            SessionMessage::Unsubscribe(subscription),
            SessionMessage::Resync(subscription),
            SessionMessage::Close(CloseReason::ClientClosed),
            SessionMessage::Goodbye(CloseReason::Protocol {
                detail: "版本不匹配".to_owned(),
            }),
        ],
    };
    assert_eq!(
        mineral_protocol::Event::Failure(FailureNotice::QueueTransformFailed).subscription(),
        Subscription::Toast
    );
    json_round_trips(&batch)?;
    framed_round_trips(batch.clone()).await?;
    // length-delimited 字节编码直接过一遍(与 SocketWire 同路径)。
    let bytes = encode(&batch)?;
    let back: MessageBatch = decode(&bytes)?;
    assert_eq!(format!("{back:?}"), format!("{batch:?}"));
    Ok(())
}

/// Player subscriptions preserve local paths containing non-UTF-8 bytes.
#[cfg(unix)]
#[tokio::test]
async fn player_update_preserves_non_unicode_media_path() -> color_eyre::Result<()> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

    use mineral_model::{AudioFormat, DirectMedia, PlaybackMediaInfo, SourceKind};
    use mineral_protocol::CurrentSync;

    let path = PathBuf::from(OsString::from_vec(b"/music/track-\xff.wav".to_vec()));
    let song_id = SongId::new(SourceKind::LOCAL, "native-path");
    let media = DirectMedia::local(
        PlaybackMediaInfo {
            song_id,
            bitrate_bps: None,
            size: None,
            format: Some(AudioFormat::Wav),
            bit_depth: Some(16),
            substituted: false,
        },
        path.clone(),
    );
    let message = SessionMessage::Update(UpdateEnvelope {
        subscription: SubscriptionId::new(1),
        version: 1,
        parts: 1,
        index: 0,
        payload: UpdatePayload::Player {
            sync: Box::new(PlayerSync {
                current: Some(CurrentSync {
                    direct_media: Some(media),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            queue_parts: 1,
        },
    });
    json_round_trips(&message)?;
    framed_round_trips(message.clone()).await?;
    let back = decode::<SessionMessage>(&encode(&message)?)?;
    let SessionMessage::Update(UpdateEnvelope {
        payload: UpdatePayload::Player { sync, .. },
        ..
    }) = back
    else {
        return Err(eyre!("missing player update"));
    };
    let media = sync
        .current
        .and_then(|current| current.direct_media)
        .ok_or_else(|| eyre!("missing current media"))?;
    assert_eq!(media.locator().local_path(), Some(path.as_path()));
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

/// bincode 字节与不完整负载的错误类别均由 codec 自己承担。
#[test]
fn codec_bytes_and_error_source() -> color_eyre::Result<()> {
    assert_eq!(
        encode(&0x1234_5678_u32)?.as_ref(),
        &[0x78, 0x56, 0x34, 0x12]
    );
    let error = decode::<u32>(&[0x78])
        .err()
        .ok_or_else(|| eyre!("负载字节不完整时应拒绝解码"))?;
    assert!(matches!(error, CodecError::Decode(_)));
    assert!(std::error::Error::source(&error).is_some());
    Ok(())
}

/// 读取和写入超过上限的帧均报告帧长问题，socket 承载保留同样的类别。
#[tokio::test]
async fn oversized_frame_is_classified() -> color_eyre::Result<()> {
    let max = tokio_util::codec::LengthDelimitedCodec::new().max_frame_length();
    let (a, _b) = duplex(64);
    let mut sender = framed(a);
    let error = send(&mut sender, &vec![0_u8; max])
        .await
        .err()
        .ok_or_else(|| eyre!("超长帧不应发送成功"))?;
    assert!(matches!(
        error,
        CodecError::FrameTooLarge { max: limit, .. } if limit == max
    ));
    assert!(std::error::Error::source(&error).is_some());

    let (mut writer, reader) = duplex(64);
    writer
        .write_all(&u32::try_from(max + 1)?.to_be_bytes())
        .await?;
    let mut receiver = framed(reader);
    let error = recv::<u8, _>(&mut receiver)
        .await
        .err()
        .ok_or_else(|| eyre!("超长帧不应接收成功"))?;
    assert!(matches!(
        error,
        CodecError::FrameTooLarge { max: limit, .. } if limit == max
    ));

    let (mut writer, reader) = tokio::net::UnixStream::pair()?;
    writer
        .write_all(&u32::try_from(max + 1)?.to_be_bytes())
        .await?;
    let mut wire = SocketWire::from_stream(reader);
    let error = wire
        .recv()
        .await
        .err()
        .ok_or_else(|| eyre!("socket 不应接收超长帧"))?;
    assert!(matches!(
        error,
        WireError::Protocol {
            source: CodecError::FrameTooLarge { max: limit, .. },
            ..
        } if limit == max
    ));

    let (_reader, writer) = tokio::net::UnixStream::pair()?;
    let mut wire = SocketWire::from_stream(writer);
    let batch = MessageBatch::one(SessionMessage::Hello(ClientInfo::new(&"x".repeat(max))));
    let error = wire
        .send(batch)
        .await
        .err()
        .ok_or_else(|| eyre!("socket 不应发送超长帧"))?;
    assert!(matches!(
        error,
        WireError::Protocol {
            source: CodecError::FrameTooLarge { max: limit, .. },
            ..
        } if limit == max
    ));
    Ok(())
}

/// socket 承载收到损坏的负载时保留 bincode 错误链。
#[tokio::test]
async fn invalid_socket_payload_is_decode_failure() -> color_eyre::Result<()> {
    let (mut writer, reader) = tokio::net::UnixStream::pair()?;
    writer.write_all(&[0, 0, 0, 1, 0xff]).await?;
    let mut wire = SocketWire::from_stream(reader);
    let error = wire
        .recv()
        .await
        .err()
        .ok_or_else(|| eyre!("socket 不应接收损坏负载"))?;
    assert!(matches!(
        error,
        WireError::Protocol {
            source: CodecError::Decode(_),
            ..
        }
    ));
    assert!(std::error::Error::source(&error).is_some());
    Ok(())
}

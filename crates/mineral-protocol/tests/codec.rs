//! 端到端 codec 测试:在 in-memory `DuplexStream` 上 framed → send → recv → 反序列化。

use color_eyre::eyre::eyre;
use mineral_audio::AudioSnapshot;
use mineral_model::{BitRate, MediaUrl, SongId, SourceKind};
use mineral_protocol::{
    CancelFilter, ChannelFetchKindTag, PlayMode, PlayerSnapshot, Request, Response, SongStatsWire,
    framed, recv, send,
};
use mineral_task::{ChannelFetchKind, Priority, Snapshot, TaskId, TaskKind};
use mineral_test::song;
use pretty_assertions::assert_eq;
use tokio::io::duplex;

/// 把一个 [`Request`] 走 framed round-trip,断言收回的与发出的 Debug 等价
/// (`Request` 不实现 `PartialEq`,但成功反序列化的 Debug 必然逐字段相同)。
async fn req_round_trips(req: Request) -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);
    let want = format!("{req:?}");
    send(&mut sender, &req).await?;
    let got: Request = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    assert_eq!(format!("{got:?}"), want);
    Ok(())
}

/// 同 [`req_round_trips`],[`Response`] 版。
async fn resp_round_trips(resp: Response) -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);
    let want = format!("{resp:?}");
    send(&mut sender, &resp).await?;
    let got: Response = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    assert_eq!(format!("{got:?}"), want);
    Ok(())
}

#[tokio::test]
async fn round_trip_request_play() -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);

    let url = MediaUrl::remote("https://example.com/song.mp3")?;
    send(&mut sender, &Request::Play(url.clone())).await?;
    let got: Request = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    if let Request::Play(u) = got {
        assert_eq!(u, url);
    } else {
        return Err(eyre!("unexpected variant: {got:?}"));
    }
    Ok(())
}

#[tokio::test]
async fn round_trip_request_submit_task() -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);

    let kind = TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
        song_id: SongId::new(SourceKind::NETEASE, "12345"),
        quality: BitRate::Higher,
    });
    send(
        &mut sender,
        &Request::SubmitTask(kind.clone(), Priority::User),
    )
    .await?;
    let got: Request = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    if let Request::SubmitTask(k, p) = got {
        assert_eq!(k, kind);
        assert_eq!(p, Priority::User);
    } else {
        return Err(eyre!("unexpected variant: {got:?}"));
    }
    Ok(())
}

#[tokio::test]
async fn round_trip_request_cancel_tasks() -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);

    let filter = CancelFilter::ChannelFetchKinds(vec![
        ChannelFetchKindTag::SongUrl,
        ChannelFetchKindTag::Lyrics,
    ]);
    send(&mut sender, &Request::CancelTasks(filter.clone())).await?;
    let got: Request = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    if let Request::CancelTasks(f) = got {
        assert_eq!(f, filter);
    } else {
        return Err(eyre!("unexpected variant: {got:?}"));
    }
    Ok(())
}

#[tokio::test]
async fn round_trip_response_audio_snapshot() -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);

    let snap = AudioSnapshot {
        playing: true,
        position_ms: 12_345,
        duration_ms: 200_000,
        volume_pct: 77,
        track_finished_seq: 3,
        backend: mineral_audio::AudioBackend::Null,
        download_complete: false,
        buffered_bps: 4_200,
        // gapless 字段给辨识度非默认值,roundtrip 等值断言覆盖到它们(bincode 位置式,守住没被 skip)。
        current_track_token: 9,
        next_duration_ms: 180_000,
        next_buffered_bps: 6_000,
        next_ready: true,
        next_download_complete: true,
        sample_rate_hz: 44_100,
    };
    send(&mut sender, &Response::AudioSnapshot(snap)).await?;
    let got: Response = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    if let Response::AudioSnapshot(s) = got {
        assert_eq!(s, snap);
    } else {
        return Err(eyre!("unexpected variant: {got:?}"));
    }
    Ok(())
}

#[tokio::test]
async fn round_trip_response_error() -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);

    let msg = "daemon busy: another client is connected";
    send(&mut sender, &Response::Error(msg.to_owned())).await?;
    let got: Response = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    if let Response::Error(m) = got {
        assert_eq!(m, msg);
    } else {
        return Err(eyre!("unexpected variant: {got:?}"));
    }
    Ok(())
}

#[tokio::test]
async fn req_resp_pair_over_one_stream() -> color_eyre::Result<()> {
    // 模拟同一条 socket 上 client 发 Request → server 收 → server 发 Response → client 收。
    let (client_side, server_side) = duplex(64 * 1024);
    let mut client = framed(client_side);
    let mut server = framed(server_side);

    // client → server: AudioSnapshot 请求
    send(&mut client, &Request::AudioSnapshot).await?;
    let req: Request = recv(&mut server)
        .await?
        .ok_or_else(|| eyre!("server got nothing"))?;
    assert!(matches!(req, Request::AudioSnapshot));

    // server → client: 回 snapshot
    let snap = AudioSnapshot::default();
    send(&mut server, &Response::AudioSnapshot(snap)).await?;
    let resp: Response = recv(&mut client)
        .await?
        .ok_or_else(|| eyre!("client got nothing"))?;
    assert!(matches!(resp, Response::AudioSnapshot(_)));
    Ok(())
}

/// 简单 / 单元 Request variant 的 round-trip(无 payload 或标量 payload)。
#[tokio::test]
async fn round_trip_simple_requests() -> color_eyre::Result<()> {
    req_round_trips(Request::Pause).await?;
    req_round_trips(Request::Resume).await?;
    req_round_trips(Request::Stop).await?;
    req_round_trips(Request::Seek(12_345)).await?;
    req_round_trips(Request::SetVolume(50)).await?;
    req_round_trips(Request::CyclePlayMode).await?;
    req_round_trips(Request::PrevOrRestart).await?;
    req_round_trips(Request::NextSong).await?;
    req_round_trips(Request::DrainTaskEvents).await?;
    req_round_trips(Request::TaskSnapshot).await?;
    req_round_trips(Request::PlayerSnapshot).await?;
    req_round_trips(Request::PullPcm(256)).await?;
    Ok(())
}

/// 带 Song payload 的 Request:PlaySong / SetQueue。
#[tokio::test]
async fn round_trip_song_payload_requests() -> color_eyre::Result<()> {
    req_round_trips(Request::PlaySong(Box::new(song("s1")))).await?;
    req_round_trips(Request::SetQueue {
        queue: vec![song("s1"), song("s2")],
        target_id: SongId::new(SourceKind::NETEASE, "s2"),
    })
    .await?;
    Ok(())
}

/// love / 统计相关 Request 与 Response 的 round-trip。
#[tokio::test]
async fn round_trip_love_and_stats() -> color_eyre::Result<()> {
    req_round_trips(Request::ToggleLove(SongId::new(SourceKind::NETEASE, "123"))).await?;
    req_round_trips(Request::QuerySongStats(SongId::new(
        SourceKind::NETEASE,
        "123",
    )))
    .await?;
    resp_round_trips(Response::LoveToggled(true)).await?;
    resp_round_trips(Response::SongStats(Some(SongStatsWire {
        play_count: 3,
        skip_count: 1,
        total_listen_ms: 500_000,
        last_played_at: Some(1_700_000_000_000),
        loved: true,
    })))
    .await?;
    resp_round_trips(Response::SongStats(None)).await?;
    Ok(())
}

/// Response variant 的 round-trip:Ok / TaskId / TaskEvents / TaskSnapshot / PcmData。
#[tokio::test]
async fn round_trip_responses() -> color_eyre::Result<()> {
    resp_round_trips(Response::Ok).await?;
    resp_round_trips(Response::TaskId(TaskId::default())).await?;
    resp_round_trips(Response::TaskEvents(Vec::new())).await?;
    resp_round_trips(Response::TaskSnapshot(Snapshot {
        running: 2,
        by_lane: Default::default(),
        by_kind: Default::default(),
    }))
    .await?;
    resp_round_trips(Response::PcmData {
        samples: vec![0.0, 0.5, -0.5],
        sample_rate: 44_100,
    })
    .await?;
    Ok(())
}

/// 含非空 queue + Shuffle + original_queue 的 PlayerSnapshot 完整往返。
#[tokio::test]
async fn round_trip_player_snapshot_rich() -> color_eyre::Result<()> {
    let snap = PlayerSnapshot {
        queue: vec![song("a"), song("b"), song("c")],
        queue_sel: 1,
        play_mode: PlayMode::Shuffle,
        original_queue: Some(vec![song("a"), song("b"), song("c")]),
        current_song: Some(song("b")),
        ..PlayerSnapshot::default()
    };
    resp_round_trips(Response::PlayerSnapshot(Box::new(snap))).await?;
    Ok(())
}

/// 属性测试:随机 `Request` 经 bincode 编/解码 Debug 恒等。覆盖手写 example 测不到的
/// 字段组合(尤其 Song-laden 的 PlaySong / SetQueue)。framing(length-delimited)是上游
/// codec,不在此重测;序列化保真才是本仓的风险点。
mod proptests {
    use bincode::{deserialize, serialize};
    use mineral_model::{SongId, SourceKind};
    use mineral_protocol::Request;
    use mineral_test::arb_song;
    use proptest::collection::vec;
    use proptest::prelude::{Just, Strategy, any, prop_oneof, proptest};
    use proptest::test_runner::TestCaseError;

    /// 随机 `Request`:unit variant + 数值 variant + Song-laden variant。
    fn arb_request() -> impl Strategy<Value = Request> {
        prop_oneof![
            Just(Request::Pause),
            Just(Request::Resume),
            Just(Request::Stop),
            Just(Request::AudioSnapshot),
            Just(Request::DrainTaskEvents),
            Just(Request::TaskSnapshot),
            Just(Request::CyclePlayMode),
            Just(Request::PrevOrRestart),
            Just(Request::NextSong),
            Just(Request::PlayerSnapshot),
            any::<u64>().prop_map(Request::Seek),
            any::<u8>().prop_map(Request::SetVolume),
            any::<usize>().prop_map(Request::PullPcm),
            arb_song().prop_map(|s| Request::PlaySong(Box::new(s))),
            (vec(arb_song(), 0..4), any::<String>()).prop_map(|(queue, target)| {
                Request::SetQueue {
                    queue,
                    target_id: SongId::new(SourceKind::NETEASE, target.as_str()),
                }
            }),
            any::<String>()
                .prop_map(|s| Request::ToggleLove(SongId::new(SourceKind::NETEASE, s.as_str()))),
            any::<String>().prop_map(|s| Request::QuerySongStats(SongId::new(
                SourceKind::NETEASE,
                s.as_str()
            ))),
        ]
    }

    proptest! {
        /// 任意 `Request` 经 bincode 往返后 Debug 恒等(`Request` 无 `PartialEq`,沿用 Debug 比较)。
        #[test]
        fn request_bincode_roundtrip(req in arb_request()) {
            let bytes = serialize(&req).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let back: Request = deserialize(&bytes).map_err(|e| TestCaseError::fail(e.to_string()))?;
            proptest::prop_assert_eq!(format!("{back:?}"), format!("{req:?}"));
        }
    }
}

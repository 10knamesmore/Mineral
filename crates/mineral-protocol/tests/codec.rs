//! 端到端 codec 测试:在 in-memory `DuplexStream` 上 framed → send → recv → 反序列化。

use mineral_audio::AudioSnapshot;
use mineral_model::{MediaUrl, SongId, SourceKind};
use mineral_protocol::{CancelFilter, ChannelFetchKindTag, Request, Response, framed, recv, send};
use mineral_task::{ChannelFetchKind, Priority, TaskKind};
use tokio::io::duplex;

#[tokio::test]
async fn round_trip_request_play() -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);

    let url = MediaUrl::remote("https://example.com/song.mp3")?;
    send(&mut sender, &Request::Play(url.clone())).await?;
    let got: Request = recv(&mut receiver).await?.expect("frame missing");
    match got {
        Request::Play(u) => assert_eq!(u, url),
        other => panic!("unexpected variant: {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn round_trip_request_submit_task() -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);

    let kind = TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
        source: SourceKind::Netease,
        song_id: SongId::new("12345".to_owned()),
    });
    send(
        &mut sender,
        &Request::SubmitTask(kind.clone(), Priority::User),
    )
    .await?;
    let got: Request = recv(&mut receiver).await?.expect("frame missing");
    match got {
        Request::SubmitTask(k, p) => {
            assert_eq!(k, kind);
            assert_eq!(p, Priority::User);
        }
        other => panic!("unexpected variant: {other:?}"),
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
    let got: Request = recv(&mut receiver).await?.expect("frame missing");
    match got {
        Request::CancelTasks(f) => assert_eq!(f, filter),
        other => panic!("unexpected variant: {other:?}"),
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
    };
    send(&mut sender, &Response::AudioSnapshot(snap)).await?;
    let got: Response = recv(&mut receiver).await?.expect("frame missing");
    match got {
        Response::AudioSnapshot(s) => assert_eq!(s, snap),
        other => panic!("unexpected variant: {other:?}"),
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
    let got: Response = recv(&mut receiver).await?.expect("frame missing");
    match got {
        Response::Error(m) => assert_eq!(m, msg),
        other => panic!("unexpected variant: {other:?}"),
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
    let req: Request = recv(&mut server).await?.expect("server got nothing");
    assert!(matches!(req, Request::AudioSnapshot));

    // server → client: 回 snapshot
    let snap = AudioSnapshot::default();
    send(&mut server, &Response::AudioSnapshot(snap)).await?;
    let resp: Response = recv(&mut client).await?.expect("client got nothing");
    assert!(matches!(resp, Response::AudioSnapshot(_)));
    Ok(())
}

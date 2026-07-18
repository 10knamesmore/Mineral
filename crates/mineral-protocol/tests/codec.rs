//! 端到端 codec 测试:在 in-memory `DuplexStream` 上 framed → send → recv → 反序列化。

use color_eyre::eyre::eyre;
use mineral_audio::AudioSnapshot;
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, BitRate, MediaUrl, Playlist, PlaylistId, SongId, SourceKind,
};
use mineral_protocol::{
    CancelFilter, ChannelFetchKindTag, CopyTemplateCtx, CurrentSync, DownloadProgress,
    DownloadTarget, KeyContext, PlayMode, PlayerSync, PlayerVersions, PlaylistRef, QueueSync,
    Request, Response, ScriptBind, SongStatsWire, StoreValue, ViewKind, framed, recv, send,
};
use mineral_task::{ChannelFetchKind, Priority, Snapshot, TaskId, TaskKind};
use mineral_test::song;
use pretty_assertions::assert_eq;
use tokio::io::duplex;

/// 同一值经 serde_json 往返,断言 Debug 保真。codec 可换的守卫(与 tests/frame.rs
/// 的 `dual_codec_roundtrip` 同约定):wire 类型只许依赖 serde derive,不许绑死
/// bincode。framed bincode 路径由调用方覆盖。
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

/// 把一个 [`Request`] 走 framed round-trip,断言收回的与发出的 Debug 等价
/// (`Request` 不实现 `PartialEq`,但成功反序列化的 Debug 必然逐字段相同);
/// 同一值顺带过 [`json_round_trips`](双 codec 一次覆盖)。
async fn req_round_trips(req: Request) -> color_eyre::Result<()> {
    json_round_trips(&req)?;
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
    json_round_trips(&resp)?;
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
    let req = Request::Play(url.clone());
    json_round_trips(&req)?;
    send(&mut sender, &req).await?;
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
    let req = Request::SubmitTask(kind.clone(), Priority::User);
    json_round_trips(&req)?;
    send(&mut sender, &req).await?;
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
    let req = Request::CancelTasks(filter.clone());
    json_round_trips(&req)?;
    send(&mut sender, &req).await?;
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
        duration_ms: Some(200_000),
        volume_pct: 77,
        track_finished_seq: 3,
        backend: mineral_audio::AudioBackend::Null,
        download_complete: false,
        buffered_bps: mineral_audio::Bps::new(4_200),
        // gapless 字段给辨识度非默认值,roundtrip 等值断言覆盖到它们(bincode 位置式,守住没被 skip)。
        current_track_token: 9,
        next_duration_ms: Some(180_000),
        next_buffered_bps: mineral_audio::Bps::new(6_000),
        next_ready: true,
        next_download_complete: true,
        sample_rate_hz: 44_100,
    };
    let resp = Response::AudioSnapshot(snap);
    json_round_trips(&resp)?;
    send(&mut sender, &resp).await?;
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
    let resp = Response::Error(msg.to_owned());
    json_round_trips(&resp)?;
    send(&mut sender, &resp).await?;
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
    req_round_trips(Request::PlayerSync(PlayerVersions {
        queue: 3,
        current: 7,
    }))
    .await?;
    req_round_trips(Request::PullPcm(256)).await?;
    req_round_trips(Request::Shutdown).await?;
    Ok(())
}

/// store 读写与脚本动作触发(含 ctx)的 round-trip。
#[tokio::test]
async fn round_trip_store_and_invoke_action() -> color_eyre::Result<()> {
    let id = SongId::new(SourceKind::NETEASE, "31");
    req_round_trips(Request::StoreGet {
        song: id.clone(),
        key: "plugin.x".to_owned(),
    })
    .await?;
    req_round_trips(Request::StoreSet {
        song: id.clone(),
        key: "plugin.x".to_owned(),
        value: StoreValue::Text("值".to_owned()),
    })
    .await?;
    req_round_trips(Request::StoreInc {
        song: id.clone(),
        key: "plugin.n".to_owned(),
        delta: -3,
    })
    .await?;
    resp_round_trips(Response::StoreValue(StoreValue::Real(2.5))).await?;
    resp_round_trips(Response::StoreValue(StoreValue::Nil)).await?;

    // CLI 触发:无 ctx,带位置实参
    req_round_trips(Request::InvokeAction {
        name: "my.skip".to_owned(),
        ctx: None,
        args: Vec::from(["mode".to_owned(), "clock".to_owned()]),
    })
    .await?;
    // TUI 触发:带按键瞬间上下文(builder 构造,getter 读;字段全 Some + 全 None 各一)
    let ctx = KeyContext::builder()
        .view(ViewKind::Tracks)
        .selected_song(Some(Box::new(song("31"))))
        .selected_playlist(Some(PlaylistRef {
            id: PlaylistId::new(SourceKind::NETEASE, "p1"),
            name: "日常".to_owned(),
        }))
        .now_playing(Some(Box::new(song("32"))))
        .selected_loved(Some(true))
        .search_query(Some("雨".to_owned()))
        .build();
    req_round_trips(Request::InvokeAction {
        name: "my.rate".to_owned(),
        ctx: Some(ctx),
        args: Vec::new(),
    })
    .await?;
    let empty = KeyContext::builder()
        .view(ViewKind::Search)
        .selected_song(None)
        .selected_playlist(None)
        .now_playing(None)
        .selected_loved(None)
        .search_query(None)
        .build();
    req_round_trips(Request::InvokeAction {
        name: "my.global".to_owned(),
        ctx: Some(empty),
        args: Vec::new(),
    })
    .await?;
    Ok(())
}

/// 脚本 bind 表拉取:请求无参,应答携带 key→动作名列表。
#[tokio::test]
async fn round_trip_script_binds() -> color_eyre::Result<()> {
    req_round_trips(Request::ScriptBinds).await?;
    resp_round_trips(Response::ScriptBinds(vec![
        ScriptBind {
            key: "X".to_owned(),
            action: "bind#1".to_owned(),
        },
        ScriptBind {
            key: "<C-g>".to_owned(),
            action: "bind#2".to_owned(),
        },
    ]))
    .await?;
    resp_round_trips(Response::ScriptBinds(Vec::new())).await?;
    Ok(())
}

/// 带 Song payload 的 Request:PlaySong / SetQueue。
#[tokio::test]
async fn round_trip_song_payload_requests() -> color_eyre::Result<()> {
    req_round_trips(Request::PlaySong(Box::new(song("s1")))).await?;
    req_round_trips(Request::SetQueue {
        queue: vec![song("s1"), song("s2")],
        target_id: SongId::new(SourceKind::NETEASE, "s2"),
        context: mineral_protocol::QueueContextWire::Search {
            query: "李志".to_owned(),
        },
    })
    .await?;
    Ok(())
}

/// love / 统计相关 Request 与 Response 的 round-trip。
#[tokio::test]
async fn round_trip_love_and_stats() -> color_eyre::Result<()> {
    req_round_trips(Request::ToggleLove(Box::new(song("123")))).await?;
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

/// 双重段齐发(Shuffle + original_queue + current)的 PlayerSync 完整往返。
#[tokio::test]
async fn round_trip_player_sync_rich() -> color_eyre::Result<()> {
    let sync = PlayerSync {
        versions: PlayerVersions {
            queue: 4,
            current: 9,
        },
        queue_sel: 1,
        play_mode: PlayMode::Shuffle,
        play_origin: None,
        queue: Some(QueueSync {
            queue: vec![song("a"), song("b"), song("c")],
            original_queue: Some(vec![song("a"), song("b"), song("c")]),
        }),
        current: Some(CurrentSync {
            current_song: Some(song("b")),
            play_url: None,
            current_lyrics: None,
            current_lyrics_song_id: None,
            // 包络随 current 段上线:points 字节与算法版本不变形。
            current_envelope: Some(mineral_model::Envelope {
                points: vec![0, 128, 255],
                version: 1,
            }),
        }),
    };
    resp_round_trips(Response::PlayerSync(Box::new(sync))).await?;
    Ok(())
}

/// 轻段-only(版本一致,两重段缺席)的 PlayerSync 往返:稳态 tick 的主路径。
#[tokio::test]
async fn round_trip_player_sync_light_only() -> color_eyre::Result<()> {
    let sync = PlayerSync {
        versions: PlayerVersions {
            queue: 4,
            current: 9,
        },
        queue_sel: 2,
        play_mode: PlayMode::RepeatAll,
        play_origin: None,
        queue: None,
        current: None,
    };
    resp_round_trips(Response::PlayerSync(Box::new(sync))).await?;
    Ok(())
}

/// 属性测试:随机 `Request` 经 bincode 编/解码 Debug 恒等。覆盖手写 example 测不到的
/// 字段组合(尤其 Song-laden 的 PlaySong / SetQueue)。framing(length-delimited)是上游
/// codec,不在此重测;序列化保真才是本仓的风险点。
mod proptests {
    use bincode::{deserialize, serialize};
    use mineral_model::{SongId, SourceKind};
    use mineral_protocol::{PlayerVersions, Request};
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
            Just(Request::Shutdown),
            (any::<u64>(), any::<u64>()).prop_map(|(queue, current)| {
                Request::PlayerSync(PlayerVersions { queue, current })
            }),
            any::<u64>().prop_map(Request::Seek),
            any::<u8>().prop_map(Request::SetVolume),
            any::<usize>().prop_map(Request::PullPcm),
            arb_song().prop_map(|s| Request::PlaySong(Box::new(s))),
            (vec(arb_song(), 0..4), any::<String>()).prop_map(|(queue, target)| {
                Request::SetQueue {
                    queue,
                    target_id: SongId::new(SourceKind::NETEASE, target.as_str()),
                    context: mineral_protocol::QueueContextWire::Unknown,
                }
            }),
            arb_song().prop_map(|s| Request::ToggleLove(Box::new(s))),
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

        /// 任意 `Request` 经 JSON 往返后 Debug 恒等(codec 可换守卫,与 bincode 版共用 arb)。
        #[test]
        fn request_json_roundtrip(req in arb_request()) {
            let json = serde_json::to_string(&req).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let back: Request = serde_json::from_str(&json).map_err(|e| TestCaseError::fail(e.to_string()))?;
            proptest::prop_assert_eq!(format!("{back:?}"), format!("{req:?}"));
        }
    }
}

/// 搜索任务 / 歌单写操作 / 队列插播 / caps 请求的 round-trip
/// (新消息覆盖:bincode 位置式编码,字段增减会在这里炸出来)。
#[tokio::test]
async fn round_trip_search_write_queue_caps() -> color_eyre::Result<()> {
    req_round_trips(Request::SubmitTask(
        TaskKind::ChannelFetch(ChannelFetchKind::Search {
            source: SourceKind::NETEASE,
            kind: mineral_model::SearchKind::Artist,
            query: String::from("海阔天空"),
            page: mineral_channel_core::Page::new(30, 30),
        }),
        Priority::User,
    ))
    .await?;
    req_round_trips(Request::SubmitTask(
        TaskKind::PlaylistWrite(mineral_task::PlaylistWriteOp::AddSongs {
            id: PlaylistId::new(SourceKind::NETEASE, "123"),
            songs: vec![SongId::new(SourceKind::NETEASE, "186016")],
        }),
        Priority::User,
    ))
    .await?;
    req_round_trips(Request::QueueInsertNext {
        song: Box::new(song("ins")),
        context: mineral_protocol::QueueContextWire::Manual,
    })
    .await?;
    req_round_trips(Request::QueueAppend {
        song: Box::new(song("app")),
        context: mineral_protocol::QueueContextWire::Search {
            query: "q".to_owned(),
        },
    })
    .await?;
    req_round_trips(Request::ChannelCaps).await?;
    resp_round_trips(Response::ChannelCaps(vec![(
        SourceKind::NETEASE,
        mineral_channel_core::ChannelCaps::builder()
            .searchable(vec![
                mineral_model::SearchKind::Song,
                mineral_model::SearchKind::Artist,
            ])
            .playlist_edit(true)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build(),
    )]))
    .await?;
    Ok(())
}

/// 下载 / 进程信息 / 复制模板 / 终端上报的 round-trip(此前无覆盖的变体补齐,
/// 双 codec 经 helper 一并守住)。
#[tokio::test]
async fn round_trip_download_info_copy_terminal() -> color_eyre::Result<()> {
    req_round_trips(Request::DaemonInfo).await?;
    resp_round_trips(Response::DaemonInfo { pid: 4242 }).await?;

    req_round_trips(Request::Download(DownloadTarget::Song(Box::new(song(
        "dl",
    )))))
    .await?;
    req_round_trips(Request::Download(DownloadTarget::Playlist(
        PlaylistId::new(SourceKind::NETEASE, "p1"),
    )))
    .await?;
    req_round_trips(Request::DownloadProgress).await?;
    resp_round_trips(Response::DownloadProgress(DownloadProgress {
        active: true,
        done: 3,
        total: 10,
        bytes_done: 1_024,
        bytes_total: 4_096,
        speed_bps: 512,
        queued: 1,
        result_seq: 2,
        last_ok: 5,
        last_skip: 1,
        last_fail: 0,
    }))
    .await?;

    // 复制模板:四种实体 ctx 各一(Playlist/Album/Artist 载荷此前从未过 wire 测试)。
    req_round_trips(Request::RenderCopyTemplate {
        index: 2,
        ctx: CopyTemplateCtx::Song(Box::new(song("cp"))),
    })
    .await?;
    req_round_trips(Request::RenderCopyTemplate {
        index: 0,
        ctx: CopyTemplateCtx::Playlist(Box::new(
            Playlist::builder()
                .id(PlaylistId::new(SourceKind::NETEASE, "pl"))
                .name("收藏夹".to_owned())
                .songs(vec![song("in-pl")])
                .build(),
        )),
    })
    .await?;
    req_round_trips(Request::RenderCopyTemplate {
        index: 1,
        ctx: CopyTemplateCtx::Album(Box::new(
            Album::builder()
                .id(AlbumId::new(SourceKind::NETEASE, "al"))
                .name("专辑".to_owned())
                .publish_time_ms(1_700_000_000_000)
                .build(),
        )),
    })
    .await?;
    req_round_trips(Request::RenderCopyTemplate {
        index: 3,
        ctx: CopyTemplateCtx::Artist(Box::new(
            Artist::builder()
                .id(ArtistId::new(SourceKind::NETEASE, "ar"))
                .name("乐队".to_owned())
                .build(),
        )),
    })
    .await?;
    resp_round_trips(Response::CopyText(Ok("标题 - 歌手".to_owned()))).await?;
    resp_round_trips(Response::CopyText(Err("模板下标越界".to_owned()))).await?;

    req_round_trips(Request::TerminalState {
        rows: 50,
        cols: 200,
        fullscreen: false,
        focused: true,
    })
    .await?;
    Ok(())
}

/// 写完结事件(成功与带错误)经 TaskEvents 响应的 round-trip。
#[tokio::test]
async fn round_trip_playlist_write_done_events() -> color_eyre::Result<()> {
    resp_round_trips(Response::TaskEvents(vec![
        mineral_task::TaskEvent::PlaylistWriteDone {
            op: mineral_task::PlaylistWriteOp::Create {
                source: SourceKind::NETEASE,
                name: String::from("新歌单"),
            },
            error: None,
        },
        mineral_task::TaskEvent::PlaylistWriteDone {
            op: mineral_task::PlaylistWriteOp::Delete {
                id: PlaylistId::new(SourceKind::NETEASE, "9"),
            },
            error: Some(mineral_task::WriteError::Api {
                code: 502,
                message: String::from("歌曲已存在"),
            }),
        },
    ]))
    .await?;
    Ok(())
}

//! Frame / Event / 握手类型的 wire 守卫:bincode 与 JSON 双 codec round-trip、
//! PropName 序列化形状、oneshot client 的握手与配对语义。

use color_eyre::eyre::eyre;
use mineral_model::{SongId, SourceKind};
use mineral_protocol::{
    BusValue, ClientInfo, Event, FinishReason, Frame, OneshotClient, PkgVersion, PropName,
    PropValue, RejectReason, Request, RequestId, Response, ServerHello, SpanAlign, SpanFg,
    Subscription, TextSpan, ToastKind, framed, recv, send,
};
use pretty_assertions::assert_eq;
use tokio::io::duplex;

/// 同一值分别经 bincode 与 serde_json 往返,断言两条管道都 Debug 保真。
/// 这是「codec 可换」的守卫:wire 类型只许依赖 serde derive,不许绑死 bincode。
fn dual_codec_roundtrip<T>(value: &T) -> color_eyre::Result<()>
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let want = format!("{value:?}");
    let bin = bincode::serialize(value)?;
    let from_bin: T = bincode::deserialize(&bin)?;
    assert_eq!(format!("{from_bin:?}"), want, "bincode 往返应保真");
    let json = serde_json::to_string(value)?;
    let from_json: T = serde_json::from_str(&json)?;
    assert_eq!(format!("{from_json:?}"), want, "JSON 往返应保真");
    Ok(())
}

/// 把一个 [`Frame`] 走 framed round-trip,断言收回的与发出的 Debug 等价
/// (`Frame` 不实现 `PartialEq`,沿用 codec.rs 的 Debug 比较约定)。
async fn frame_round_trips(frame: Frame) -> color_eyre::Result<()> {
    let (a, b) = duplex(64 * 1024);
    let mut sender = framed(a);
    let mut receiver = framed(b);
    let want = format!("{frame:?}");
    send(&mut sender, &frame).await?;
    let got: Frame = recv(&mut receiver)
        .await?
        .ok_or_else(|| eyre!("frame missing"))?;
    assert_eq!(format!("{got:?}"), want);
    Ok(())
}

/// 一条样例 Toast 事件(带 id,顶替语义载荷)。
fn toast_event() -> Event {
    Event::Toast {
        kind: ToastKind::Warn,
        content: vec![TextSpan::plain("音量 42")],
        id: Some("vol".to_owned()),
        ttl_secs: None,
    }
}

/// 一条样例 Card 事件:标题 + 两行 body,第二行带样式 spans(角色色 + RGB + 修饰位)。
fn card_event() -> Event {
    Event::Card {
        kind: ToastKind::Warn,
        id: Some("release.notes".to_owned()),
        title: vec![TextSpan::plain("v0.9.0 要点")],
        ttl_secs: None,
        body: vec![
            vec![TextSpan::plain("新增配置 toast.position")],
            vec![
                TextSpan::plain("旧键 "),
                TextSpan {
                    text: "search.style".to_owned(),
                    fg: Some(SpanFg::Accent),
                    bold: true,
                    italic: false,
                    underline: false,
                    dim: false,
                    align: SpanAlign::Center,
                },
                TextSpan {
                    text: " 改名".to_owned(),
                    fg: Some(SpanFg::Rgb(0xcc, 0x66, 0x00)),
                    bold: false,
                    italic: true,
                    underline: true,
                    dim: true,
                    align: SpanAlign::Right,
                },
            ],
        ],
    }
}

#[tokio::test]
async fn round_trip_handshake_frame() -> color_eyre::Result<()> {
    frame_round_trips(Frame::Handshake(ClientInfo::new(vec![
        Subscription::Toast,
        Subscription::Property,
    ])))
    .await
}

#[tokio::test]
async fn round_trip_hello_accept_and_reject() -> color_eyre::Result<()> {
    frame_round_trips(Frame::Hello(ServerHello::accept())).await?;
    frame_round_trips(Frame::Hello(ServerHello::reject(
        RejectReason::VersionMismatch,
    )))
    .await?;
    Ok(())
}

#[tokio::test]
async fn round_trip_request_response_frames() -> color_eyre::Result<()> {
    frame_round_trips(Frame::Request {
        id: RequestId::new(7),
        req: Request::Pause,
    })
    .await?;
    frame_round_trips(Frame::Response {
        id: RequestId::new(7),
        resp: Box::new(Response::Ok),
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn round_trip_event_variants() -> color_eyre::Result<()> {
    frame_round_trips(Frame::Event(toast_event())).await?;
    frame_round_trips(Frame::Event(Event::Toast {
        kind: ToastKind::Info,
        content: vec![TextSpan::plain("一次性提示")],
        id: None,
        ttl_secs: None,
    }))
    .await?;
    frame_round_trips(Frame::Event(card_event())).await?;
    frame_round_trips(Frame::Event(Event::Card {
        kind: ToastKind::Info,
        id: None,
        title: Vec::new(),
        body: Vec::new(),
        ttl_secs: Some(8),
    }))
    .await?;
    frame_round_trips(Frame::Event(Event::PropertyChanged {
        prop: PropName::PLAYER_VOLUME,
        value: PropValue::Int(42),
    }))
    .await?;
    frame_round_trips(Frame::Event(Event::TrackFinished {
        song_id: SongId::new(SourceKind::NETEASE, "123"),
        reason: FinishReason::Eof,
    }))
    .await?;
    frame_round_trips(Frame::Event(Event::DownloadCompleted {
        song_id: SongId::new(SourceKind::NETEASE, "456"),
    }))
    .await?;
    frame_round_trips(Frame::Event(Event::StoreChanged {
        song_id: SongId::new(SourceKind::NETEASE, "789"),
        key: "plugin.skipcount".to_owned(),
    }))
    .await?;
    frame_round_trips(Frame::Event(Event::ConfigChanged {
        config: BusValue::Map(vec![(
            "tui".to_owned(),
            BusValue::Map(vec![("volume".to_owned(), BusValue::Int(80))]),
        )]),
    }))
    .await?;
    frame_round_trips(Frame::Event(Event::WindowTitleOverride {
        text: Some("⏸ 歌名".to_owned()),
    }))
    .await?;
    frame_round_trips(Frame::Event(Event::WindowTitleOverride { text: None })).await?;
    frame_round_trips(Frame::Event(Event::DismissToast {
        id: "config.reload".to_owned(),
    }))
    .await?;
    frame_round_trips(Frame::Event(Event::Task(Box::new(
        mineral_task::TaskEvent::LibrarySnapshot {
            playlists: Vec::new(),
        },
    ))))
    .await?;
    Ok(())
}

/// 双 codec 守卫:Event 各变体与握手类型经 bincode / JSON 都保真。
#[test]
fn dual_codec_event_and_handshake() -> color_eyre::Result<()> {
    dual_codec_roundtrip(&toast_event())?;
    dual_codec_roundtrip(&card_event())?;
    dual_codec_roundtrip(&Event::PropertyChanged {
        prop: PropName::PLAYER_STATE,
        value: PropValue::Str("playing".to_owned()),
    })?;
    dual_codec_roundtrip(&Event::TrackFinished {
        song_id: SongId::new(SourceKind::NETEASE, "1"),
        reason: FinishReason::Skip,
    })?;
    dual_codec_roundtrip(&Event::DownloadCompleted {
        song_id: SongId::new(SourceKind::NETEASE, "2"),
    })?;
    dual_codec_roundtrip(&Event::BusMessage {
        name: "my.refresh".to_owned(),
        payload: BusValue::Map(vec![
            ("ok".to_owned(), BusValue::Bool(true)),
            ("n".to_owned(), BusValue::Int(-3)),
            ("f".to_owned(), BusValue::Float(2.5)),
            ("s".to_owned(), BusValue::Str("文".to_owned())),
            (
                "list".to_owned(),
                BusValue::Array(vec![BusValue::Nil, BusValue::Int(1)]),
            ),
        ]),
    })?;
    dual_codec_roundtrip(&Event::PropertyChanged {
        prop: PropName::TERMINAL,
        value: PropValue::Table(vec![
            ("rows".to_owned(), PropValue::Int(50)),
            ("cols".to_owned(), PropValue::Int(220)),
            ("fullscreen".to_owned(), PropValue::Bool(true)),
        ]),
    })?;
    dual_codec_roundtrip(&Event::ConfigChanged {
        config: BusValue::Map(vec![("volume".to_owned(), BusValue::Int(80))]),
    })?;
    dual_codec_roundtrip(&Event::WindowTitleOverride {
        text: Some("⏸ 歌名".to_owned()),
    })?;
    dual_codec_roundtrip(&Event::DismissToast {
        id: "config.reload".to_owned(),
    })?;
    dual_codec_roundtrip(&ClientInfo::new(vec![Subscription::Lifecycle]))?;
    dual_codec_roundtrip(&ServerHello::reject(RejectReason::VersionMismatch))?;
    dual_codec_roundtrip(&Event::Task(Box::new(
        mineral_task::TaskEvent::LikedSongIdsFetched {
            source: SourceKind::NETEASE,
            ids: [SongId::new(SourceKind::NETEASE, "7")]
                .into_iter()
                .collect(),
        },
    )))?;
    dual_codec_roundtrip(&Event::Task(Box::new(
        mineral_task::TaskEvent::PlaylistWriteDone {
            op: mineral_task::PlaylistWriteOp::Create {
                source: SourceKind::NETEASE,
                name: "新歌单".to_owned(),
            },
            error: Some(mineral_task::WriteError::RateLimited),
        },
    )))?;
    dual_codec_roundtrip(&Event::Task(Box::new(
        mineral_task::TaskEvent::PlaylistWriteDone {
            op: mineral_task::PlaylistWriteOp::Delete {
                id: mineral_model::PlaylistId::new(SourceKind::NETEASE, "9"),
            },
            error: Some(mineral_task::WriteError::Api {
                code: 502,
                message: "歌曲已存在".to_owned(),
            }),
        },
    )))?;
    Ok(())
}

/// PropName 的 JSON 形状是裸字符串(仿 SourceKind:序列化只写 name)。
#[test]
fn prop_name_json_is_bare_string() -> color_eyre::Result<()> {
    let json = serde_json::to_string(&PropName::PLAYER_VOLUME)?;
    assert_eq!(json, "\"player.volume\"");
    let back: PropName = serde_json::from_str(&json)?;
    assert_eq!(back, PropName::PLAYER_VOLUME);
    Ok(())
}

/// 未知属性名反序列化 intern 成新 PropName,不报错、name 保真(开放命名空间)。
#[test]
fn prop_name_unknown_interns() -> color_eyre::Result<()> {
    let back: PropName = serde_json::from_str("\"plugin.custom\"")?;
    assert_eq!(back.as_str(), "plugin.custom");
    // 同名再解析,身份一致(可当 HashMap key)。
    let again: PropName = serde_json::from_str("\"plugin.custom\"")?;
    assert_eq!(back, again);
    Ok(())
}

/// 内置常量经 from_name 往返命中同一身份。
#[test]
fn prop_name_from_name_roundtrips_builtins() {
    for name in [
        PropName::PLAYER_SONG,
        PropName::PLAYER_STATE,
        PropName::PLAYER_VOLUME,
        PropName::PLAYER_POSITION,
        PropName::PLAYER_MODE,
        PropName::QUEUE_LENGTH,
        PropName::TERMINAL,
    ] {
        assert_eq!(PropName::from_name(name.as_str()), name);
    }
}

/// Event → 订阅类别的映射:server 端按它过滤下发。
#[test]
fn event_subscription_mapping() {
    assert_eq!(toast_event().subscription(), Subscription::Toast);
    assert_eq!(
        card_event().subscription(),
        Subscription::Toast,
        "卡片与 flash 同属 Toast 订阅类别,client 一次握手全收"
    );
    assert_eq!(
        Event::PropertyChanged {
            prop: PropName::QUEUE_LENGTH,
            value: PropValue::Int(3),
        }
        .subscription(),
        Subscription::Property
    );
    assert_eq!(
        Event::TrackFinished {
            song_id: SongId::new(SourceKind::NETEASE, "1"),
            reason: FinishReason::Stop,
        }
        .subscription(),
        Subscription::Lifecycle
    );
    assert_eq!(
        Event::DownloadCompleted {
            song_id: SongId::new(SourceKind::NETEASE, "2"),
        }
        .subscription(),
        Subscription::Lifecycle
    );
    assert_eq!(
        Event::BusMessage {
            name: "my.x".to_owned(),
            payload: BusValue::Nil,
        }
        .subscription(),
        Subscription::Bus
    );
    assert_eq!(
        Event::ConfigChanged {
            config: BusValue::Nil,
        }
        .subscription(),
        Subscription::Config
    );
    assert_eq!(
        Event::WindowTitleOverride { text: None }.subscription(),
        Subscription::WindowTitle
    );
    assert_eq!(
        Event::DismissToast {
            id: "config.reload".to_owned(),
        }
        .subscription(),
        Subscription::Toast
    );
    assert_eq!(
        Event::Task(Box::new(mineral_task::TaskEvent::LibrarySnapshot {
            playlists: Vec::new(),
        }))
        .subscription(),
        Subscription::Task
    );
}

/// ClientInfo::new 自动携带本端包版本,version_matches 对自身恒真。
#[test]
fn client_info_carries_pkg_version() {
    let info = ClientInfo::new(Vec::new());
    assert_eq!(info.version, PkgVersion::current());
    assert!(info.version_matches());

    let stale = ClientInfo {
        version: PkgVersion {
            major: 0,
            minor: 0,
            patch: 0,
        },
        subscriptions: Vec::new(),
    };
    assert!(!stale.version_matches(), "版本不等应判不匹配");
}

/// 版本守门策略:1.0 前不守 SemVer(必须全等),1.0 起同 major 即互通。
#[test]
fn pkg_version_compat_pre_and_post_1_0() {
    /// 简写构造。
    fn v(major: u16, minor: u16, patch: u16) -> PkgVersion {
        PkgVersion {
            major,
            minor,
            patch,
        }
    }
    // 1.0 前:任何一段不同都拒。
    assert!(v(0, 4, 2).compatible_with(v(0, 4, 2)));
    assert!(
        !v(0, 4, 2).compatible_with(v(0, 4, 3)),
        "0.x 补丁版本不同也拒"
    );
    assert!(
        !v(0, 4, 2).compatible_with(v(0, 5, 0)),
        "0.x 次版本不同也拒"
    );
    // 1.0 起:同 major 即互通,major 变才拒。
    assert!(v(1, 2, 0).compatible_with(v(1, 9, 9)), "1.x 内向后兼容");
    assert!(!v(1, 9, 9).compatible_with(v(2, 0, 0)), "major 变了拒");
    // 0.x 与 1.x 之间:走全等规则,必拒。
    assert!(!v(0, 9, 9).compatible_with(v(1, 0, 0)));
}

/// PkgVersion::current 解析自 cargo 注入的三段数字:与重组串一致、非 0.0.0、
/// Display 形如 `0.4.2`。
#[test]
fn pkg_version_current_matches_cargo_components() {
    let current = PkgVersion::current();
    let zero = PkgVersion {
        major: 0,
        minor: 0,
        patch: 0,
    };
    assert_ne!(current, zero, "版本段解析不该全部回 0 兜底");
    // 测试与被测库同 workspace 同版本,各段应与本测试进程的 cargo 注入一致。
    let want = format!(
        "{}.{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    );
    assert_eq!(current.to_string(), want);
}

/// oneshot client:握手成功后发请求,中途交错的 Event 被跳过,按 id 配对收回应答。
#[tokio::test]
async fn oneshot_pairs_response_and_skips_events() -> color_eyre::Result<()> {
    let (client_side, server_side) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut conn = framed(server_side);
        let first: Frame = recv(&mut conn)
            .await?
            .ok_or_else(|| eyre!("server 没收到握手"))?;
        let Frame::Handshake(info) = first else {
            return Err(eyre!("首帧应是 Handshake,实际 {first:?}"));
        };
        assert!(info.version_matches(), "测试两端同 build,版本应一致");
        send(&mut conn, &Frame::Hello(ServerHello::accept())).await?;

        let second: Frame = recv(&mut conn)
            .await?
            .ok_or_else(|| eyre!("server 没收到请求"))?;
        let Frame::Request { id, req } = second else {
            return Err(eyre!("应是 Request,实际 {second:?}"));
        };
        assert!(matches!(req, Request::Pause));
        // 先交错一条 Event,再回配对应答 —— client 应跳过 Event 等到 Response。
        send(&mut conn, &Frame::Event(toast_event())).await?;
        send(
            &mut conn,
            &Frame::Response {
                id,
                resp: Box::new(Response::Ok),
            },
        )
        .await?;
        Ok::<(), color_eyre::Report>(())
    });

    let mut client = OneshotClient::from_stream(client_side).await?;
    let resp = client.request(Request::Pause).await?;
    assert!(matches!(resp, Response::Ok), "应配对收回 Ok,实际 {resp:?}");
    server.await??;
    Ok(())
}

/// oneshot client:握手被拒(版本不匹配)时 from_stream 直接报人话错误。
#[tokio::test]
async fn oneshot_rejected_handshake_bails() -> color_eyre::Result<()> {
    let (client_side, server_side) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut conn = framed(server_side);
        let _first: Frame = recv(&mut conn)
            .await?
            .ok_or_else(|| eyre!("server 没收到握手"))?;
        send(
            &mut conn,
            &Frame::Hello(ServerHello::reject(RejectReason::VersionMismatch)),
        )
        .await?;
        Ok::<(), color_eyre::Report>(())
    });

    let err = match OneshotClient::from_stream(client_side).await {
        Ok(_) => return Err(eyre!("被拒的握手不该成功")),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("版本"), "错误信息应说明版本不匹配,实际:{err}");
    server.await??;
    Ok(())
}

/// 属性测试:任意 Frame / Event 经 bincode 与 JSON 往返 Debug 恒等。
mod proptests {
    use mineral_protocol::{
        ClientInfo, Event, FinishReason, Frame, PropName, PropValue, RejectReason, Request,
        RequestId, Response, ServerHello, SpanAlign, SpanFg, Subscription, TextSpan, ToastKind,
    };
    use mineral_test::arb_song;
    use proptest::option;
    use proptest::prelude::{Just, Strategy, any, prop_oneof, proptest};
    use proptest::strategy::LazyJust;
    use proptest::test_runner::TestCaseError;

    /// 随机 PropValue(四变体全覆盖)。
    fn arb_prop_value() -> impl Strategy<Value = PropValue> {
        prop_oneof![
            any::<bool>().prop_map(PropValue::Bool),
            any::<i64>().prop_map(PropValue::Int),
            any::<String>().prop_map(PropValue::Str),
            Just(PropValue::None),
        ]
    }

    /// 随机 PropName:只取内置常量(随机串会经 intern 泄漏,proptest 上千 case 不合适)。
    fn arb_prop_name() -> impl Strategy<Value = PropName> {
        prop_oneof![
            Just(PropName::PLAYER_SONG),
            Just(PropName::PLAYER_STATE),
            Just(PropName::PLAYER_VOLUME),
            Just(PropName::PLAYER_POSITION),
            Just(PropName::PLAYER_MODE),
            Just(PropName::QUEUE_LENGTH),
        ]
    }

    /// 随机 ToastKind(三变体)。
    fn arb_kind() -> impl Strategy<Value = ToastKind> {
        prop_oneof![
            Just(ToastKind::Info),
            Just(ToastKind::Warn),
            Just(ToastKind::Error)
        ]
    }

    /// 随机 SpanFg:全部主题角色 + 任意 RGB。
    fn arb_span_fg() -> impl Strategy<Value = SpanFg> {
        prop_oneof![
            Just(SpanFg::Text),
            Just(SpanFg::Subtext),
            Just(SpanFg::Overlay),
            Just(SpanFg::Accent),
            Just(SpanFg::Red),
            Just(SpanFg::Yellow),
            Just(SpanFg::Green),
            Just(SpanFg::Peach),
            any::<(u8, u8, u8)>().prop_map(|(r, g, b)| SpanFg::Rgb(r, g, b)),
        ]
    }

    /// 随机 TextSpan(任意文本 + 任意样式 / 段位组合)。
    fn arb_card_span() -> impl Strategy<Value = TextSpan> {
        let align = prop_oneof![
            Just(SpanAlign::Left),
            Just(SpanAlign::Center),
            Just(SpanAlign::Right),
        ];
        (
            any::<String>(),
            option::of(arb_span_fg()),
            any::<[bool; 4]>(),
            align,
        )
            .prop_map(
                |(text, fg, [bold, italic, underline, dim], align)| TextSpan {
                    text,
                    fg,
                    bold,
                    italic,
                    underline,
                    dim,
                    align,
                },
            )
    }

    /// 随机 Event(五变体全覆盖)。
    fn arb_event() -> impl Strategy<Value = Event> {
        let reason = prop_oneof![
            Just(FinishReason::Eof),
            Just(FinishReason::Skip),
            Just(FinishReason::Error),
            Just(FinishReason::Stop),
        ];
        let line = || proptest::collection::vec(arb_card_span(), 0..4);
        let body = proptest::collection::vec(line(), 0..4);
        prop_oneof![
            (
                arb_kind(),
                line(),
                option::of(any::<String>()),
                option::of(any::<u64>()),
            )
                .prop_map(|(kind, content, id, ttl_secs)| {
                    Event::Toast {
                        kind,
                        content,
                        id,
                        ttl_secs,
                    }
                }),
            (
                arb_kind(),
                option::of(any::<String>()),
                line(),
                body,
                option::of(any::<u64>()),
            )
                .prop_map(|(kind, id, title, body, ttl_secs)| Event::Card {
                    kind,
                    id,
                    title,
                    body,
                    ttl_secs,
                }),
            (arb_prop_name(), arb_prop_value())
                .prop_map(|(prop, value)| Event::PropertyChanged { prop, value }),
            (arb_song(), reason).prop_map(|(s, reason)| Event::TrackFinished {
                song_id: s.id,
                reason,
            }),
            arb_song().prop_map(|s| Event::DownloadCompleted { song_id: s.id }),
        ]
    }

    /// 随机 Frame(五变体全覆盖;Request/Response 取轻量样本,重载荷已被
    /// codec.rs 的 `request_bincode_roundtrip` 覆盖)。`Frame` 非 `Clone`,
    /// 固定值变体用 [`LazyJust`] 构造。
    fn arb_frame() -> impl Strategy<Value = Frame> {
        let subs = proptest::collection::vec(
            prop_oneof![
                Just(Subscription::Property),
                Just(Subscription::Toast),
                Just(Subscription::Lifecycle),
            ],
            0..4,
        );
        let reject = Just(RejectReason::VersionMismatch);
        prop_oneof![
            subs.prop_map(|s| Frame::Handshake(ClientInfo::new(s))),
            LazyJust::new(|| Frame::Hello(ServerHello::accept())),
            reject.prop_map(|r| Frame::Hello(ServerHello::reject(r))),
            any::<u64>().prop_map(|id| Frame::Request {
                id: RequestId::new(id),
                req: Request::Pause,
            }),
            any::<u64>().prop_map(|id| Frame::Response {
                id: RequestId::new(id),
                resp: Box::new(Response::Ok),
            }),
            arb_event().prop_map(Frame::Event),
        ]
    }

    proptest! {
        /// 任意 Frame 经 bincode 往返 Debug 恒等。
        #[test]
        fn frame_bincode_roundtrip(frame in arb_frame()) {
            let bytes = bincode::serialize(&frame).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let back: Frame = bincode::deserialize(&bytes).map_err(|e| TestCaseError::fail(e.to_string()))?;
            proptest::prop_assert_eq!(format!("{back:?}"), format!("{frame:?}"));
        }

        /// 任意 Frame 经 JSON 往返 Debug 恒等(codec 可换守卫)。
        #[test]
        fn frame_json_roundtrip(frame in arb_frame()) {
            let json = serde_json::to_string(&frame).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let back: Frame = serde_json::from_str(&json).map_err(|e| TestCaseError::fail(e.to_string()))?;
            proptest::prop_assert_eq!(format!("{back:?}"), format!("{frame:?}"));
        }
    }
}

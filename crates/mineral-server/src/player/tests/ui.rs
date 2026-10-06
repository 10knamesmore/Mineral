//! 配置宿主(覆盖合成 / 校验剔除)· terminal 属性上报 · queue 插入编辑。

use std::sync::Arc;

use crate::persistence::ServerStore;
use mineral_channel_core::MusicChannel;
use mineral_model::SourceKind;
use mineral_protocol::PlayMode;
use mineral_test::song;
use pretty_assertions::assert_eq;

use super::backends::RecordingChannel;
use super::fixtures::{core_with, core_with_events, core_with_events_stats};
use crate::media_cache::MediaCache;
use crate::player::PlayerCore;

/// 造一个带 event hub 接收端的 core(配置宿主 / 属性下发断言用)。
fn core_with_hub() -> color_eyre::Result<(
    PlayerCore,
    tokio::sync::broadcast::Receiver<mineral_protocol::Event>,
)> {
    let (events_tx, events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 16);
    let core = core_with_events(
        Vec::new(),
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        events_tx,
        /*script*/ None,
    )?;
    Ok((core, events_rx))
}

/// 接收服务能力变化；测试不读取 daemon 的私有配置树。
fn service_update(
    events_rx: &mut tokio::sync::broadcast::Receiver<mineral_protocol::Event>,
) -> color_eyre::Result<mineral_protocol::ServiceInfo> {
    match events_rx.try_recv()? {
        mineral_protocol::Event::ServiceInfoChanged { info } => Ok(info),
        other => {
            color_eyre::eyre::bail!("expected service capability update, received {other:?}");
        }
    }
}

/// RPC failures preserve their business category without inspecting error text.
#[tokio::test]
async fn rpc_failure_categories_follow_operation_state() -> color_eyre::Result<()> {
    use mineral_protocol::{DownloadId, FailureKind, OperationResult, Request};

    let (core, _events_rx) = core_with_hub()?;
    let client = crate::ClientHandle::new(core);

    let stop = crate::ipc::dispatch::execute_sync(
        &client,
        Request::StopDownload(DownloadId::new("missing".to_owned())),
    );
    assert!(
        matches!(stop, OperationResult::Failed(failure) if failure.kind == FailureKind::NotFound)
    );

    let store = crate::ipc::dispatch::execute_async(
        &client,
        crate::ipc::dispatch::AsyncRequest::StoreSet {
            song: mineral_model::SongId::new(SourceKind::NETEASE, "song"),
            key: "rating".to_owned(),
            value: mineral_protocol::StoreValue::Int(1),
        },
    )
    .await;
    assert!(
        matches!(store, OperationResult::Failed(failure) if failure.kind == FailureKind::Invalid)
    );
    Ok(())
}

/// 服务能力查询与订阅重放一致；无脚本或 stats.db 时不宣称能力可用。
#[tokio::test]
async fn service_info_request_matches_subscription_replay() -> color_eyre::Result<()> {
    use mineral_protocol::{Event, OperationResult, Request, Response, Subscription};
    let (core, _events_rx) = core_with_hub()?;
    let client = crate::ClientHandle::new(core);
    let OperationResult::Query(response) =
        crate::ipc::dispatch::execute_sync(&client, Request::ServiceInfo)
    else {
        color_eyre::eyre::bail!("service query missing response");
    };
    let Response::ServiceInfo(info) = *response else {
        color_eyre::eyre::bail!("service query returned wrong response");
    };
    assert!(info.queue_transforms.is_empty());
    assert!(!info.play_counts.enabled);
    assert_eq!(
        client.replay_frames(&[Subscription::ServiceInfo]).await,
        vec![Event::ServiceInfoChanged { info }]
    );
    Ok(())
}

/// 非能力配置不下发；统计策略覆盖、撤销与重载只发布实际能力变化。
#[tokio::test]
async fn daemon_policy_overrides_publish_only_capability_changes() -> color_eyre::Result<()> {
    use mineral_protocol::BusValue;
    let (core, mut events_rx) = core_with_hub()?;
    core.apply_config_overrides(vec![
        override_op("audio.volume", Some(BusValue::Int(42))),
        override_op("heartbeat_secs", Some(BusValue::Int(7))),
        override_op("prev_restart_threshold_ms", Some(BusValue::Int(1000))),
    ]);
    assert!(events_rx.try_recv().is_err());
    let excluded = BusValue::Array(vec![BusValue::Str("netease".to_owned())]);
    core.apply_config_overrides(vec![override_op(
        "stats.exclude_sources",
        Some(excluded.clone()),
    )]);
    assert_eq!(
        service_update(&mut events_rx)?.play_counts.excluded_sources,
        vec!["netease"]
    );
    core.apply_config_overrides(vec![override_op("stats.exclude_sources", Some(excluded))]);
    core.apply_config_overrides(vec![override_op("stats.collect.searches", None)]);
    assert!(events_rx.try_recv().is_err());
    core.set_config_base(mineral_config::merge_tree(
        crate::config::default_daemon_tree()?,
        serde_json::json!({"stats": {"exclude_sources": ["qqmusic"]}}),
    ));
    assert_eq!(
        core.service_info().play_counts.excluded_sources,
        vec!["netease"]
    );
    assert!(events_rx.try_recv().is_err());
    core.apply_config_overrides(vec![override_op("stats.exclude_sources", None)]);
    assert_eq!(
        service_update(&mut events_rx)?.play_counts.excluded_sources,
        vec!["qqmusic"]
    );
    assert!(events_rx.try_recv().is_err());
    Ok(())
}

/// 造一条覆盖叶子 op(测试简写)。
fn override_op(
    path: &str,
    value: Option<mineral_protocol::BusValue>,
) -> mineral_script::ConfigOverrideOp {
    mineral_script::ConfigOverrideOp {
        path: path.to_owned(),
        value,
    }
}

/// 一批覆盖只发布最终服务能力；坏 daemon 叶子和 client 配置字段均被拒绝。
#[tokio::test]
async fn daemon_override_batch_rejects_invalid_and_client_fields() -> color_eyre::Result<()> {
    use mineral_protocol::{BusValue, Event, FailureNotice};
    let (core, mut events_rx) = core_with_hub()?;
    core.apply_config_overrides(vec![
        override_op(
            "stats.exclude_sources",
            Some(BusValue::Array(vec![BusValue::Str("netease".to_owned())])),
        ),
        override_op(
            "download.max_concurrent",
            Some(BusValue::Str("invalid".to_owned())),
        ),
    ]);
    assert!(
        matches!(events_rx.try_recv()?, Event::Failure(FailureNotice::ConfigOverrideRejected { path }) if path == "download.max_concurrent")
    );
    assert_eq!(
        service_update(&mut events_rx)?.play_counts.excluded_sources,
        vec!["netease"]
    );
    assert!(events_rx.try_recv().is_err());
    let (core, mut events_rx) = core_with_hub()?;
    for rejected in [
        "behavior.volume_step",
        "tui.behavior.volume_step",
        "daemon.heartbeat_secs",
    ] {
        core.apply_config_overrides(vec![override_op(rejected, Some(BusValue::Int(5)))]);
        assert!(
            matches!(events_rx.try_recv()?, Event::Failure(FailureNotice::ConfigOverrideRejected { path }) if path == rejected)
        );
        assert!(events_rx.try_recv().is_err());
    }
    Ok(())
}

/// terminal 属性:上报后 check_props 下发 Table,断开清除后回 None。
#[tokio::test]
async fn terminal_prop_follows_report_and_clear() -> color_eyre::Result<()> {
    use mineral_protocol::Event;
    let (core, mut events_rx) = core_with_hub()?;
    core.set_terminal_state(
        /*conn*/ 0,
        crate::props::TerminalReport {
            rows: 50,
            cols: 220,
            fullscreen: true,
            focused: true,
        },
    );
    core.check_props();
    let terminal_of = |rx: &mut tokio::sync::broadcast::Receiver<Event>| {
        // check_props 首轮全量产出,滤出 terminal 一项。
        let mut found = None;
        while let Ok(ev) = rx.try_recv() {
            if let Event::PropertyChanged { prop, value } = ev
                && prop == mineral_protocol::PropName::TERMINAL
            {
                found = Some(value);
            }
        }
        found
    };
    assert_eq!(
        terminal_of(&mut events_rx),
        Some(mineral_protocol::PropValue::Table(vec![
            ("rows".to_owned(), mineral_protocol::PropValue::Int(50)),
            ("cols".to_owned(), mineral_protocol::PropValue::Int(220)),
            (
                "fullscreen".to_owned(),
                mineral_protocol::PropValue::Bool(true)
            ),
            (
                "focused".to_owned(),
                mineral_protocol::PropValue::Bool(true)
            ),
        ]))
    );
    // 值不变:下一 tick 不再下发。
    core.check_props();
    assert_eq!(terminal_of(&mut events_rx), None, "同值不得重复下发");
    // 断开清除:回 None。
    core.clear_terminal_state(/*conn*/ 0);
    core.check_props();
    assert_eq!(
        terminal_of(&mut events_rx),
        Some(mineral_protocol::PropValue::None),
        "断开后 terminal 属性应回 None"
    );
    Ok(())
}

/// set_terminal_state 的 fullscreen 翻转检测(驱动 fullscreen_changes 埋点):首次上报
/// 无前态不算切换;翻转给新值;等值上报不重复;翻回给新值。
#[tokio::test]
async fn set_terminal_state_flags_fullscreen_toggle() -> color_eyre::Result<()> {
    let (core, _rx) = core_with_hub()?;
    let report = |fullscreen: bool| crate::props::TerminalReport {
        rows: 40,
        cols: 100,
        fullscreen,
        focused: true,
    };
    assert_eq!(
        core.set_terminal_state(/*conn*/ 0, report(/*fullscreen*/ false)),
        None,
        "首次上报无前态,不算切换"
    );
    assert_eq!(
        core.set_terminal_state(/*conn*/ 0, report(/*fullscreen*/ true)),
        Some(true),
        "翻到全屏应给新值"
    );
    assert_eq!(
        core.set_terminal_state(/*conn*/ 0, report(/*fullscreen*/ true)),
        None,
        "等值上报(每 tick)不得重复触发"
    );
    assert_eq!(
        core.set_terminal_state(/*conn*/ 0, report(/*fullscreen*/ false)),
        Some(false),
        "翻回非全屏应给新值"
    );
    // 别的连接翻转不算本连接的切换(fullscreen 前态 per-conn 归属)。
    assert_eq!(
        core.set_terminal_state(/*conn*/ 1, report(/*fullscreen*/ true)),
        None,
        "另一连接首次上报,不算切换"
    );
    Ok(())
}

/// 多终端 last-wins:`terminal` 属性取最近上报的连接;最近者断开回落到
/// 次近;全部断开回 None。
#[test]
fn terminal_states_last_wins_and_fallback() {
    let mut states = crate::props::TerminalStates::default();
    let report = |rows: u16| crate::props::TerminalReport {
        rows,
        cols: 100,
        fullscreen: false,
        focused: true,
    };
    states.set(/*conn*/ 1, report(24));
    states.set(/*conn*/ 2, report(50));
    assert_eq!(
        states.current().map(|t| t.rows),
        Some(50),
        "最近上报者(conn 2)生效"
    );
    states.set(/*conn*/ 1, report(30));
    assert_eq!(
        states.current().map(|t| t.rows),
        Some(30),
        "conn 1 再上报即顶替(无主终端,谁新谁生效)"
    );
    states.remove(/*conn*/ 1);
    assert_eq!(
        states.current().map(|t| t.rows),
        Some(50),
        "最近者断开,回落到次近(conn 2)"
    );
    states.remove(/*conn*/ 2);
    assert_eq!(states.current(), None, "全部离线回 None");
}

/// 整组插播与追加保序、不去重、不改当前曲；每组只发布一次，shuffle 原序同步。
#[tokio::test]
async fn queue_insert_next_and_append_keep_current() -> color_eyre::Result<()> {
    let core = core_with(Arc::default())?;
    core.replace_queue(
        vec![song("a"), song("b")],
        0,
        mineral_stats::QueueContext::Unknown,
    )?;
    let version = core.with_state(|st| {
        st.current_song = Some(song("a"));
        st.prefetch_vetoed = vec![1];
        st.queue_version
    });
    assert!(core.queue_insert_next(
        vec![song("c"), song("c2"), song("c")],
        mineral_stats::QueueContext::Manual,
    ));
    core.with_state(|st| {
        assert_eq!(st.queue_version, version.next());
        assert!(st.prefetch_vetoed.is_empty());
    });
    assert!(core.queue_append(
        vec![song("d"), song("d2"), song("d")],
        mineral_stats::QueueContext::Manual,
    ));
    core.with_state(|st| {
        assert_eq!(
            st.queue,
            vec![
                song("a"),
                song("c"),
                song("c2"),
                song("c"),
                song("b"),
                song("d"),
                song("d2"),
                song("d")
            ]
        );
        assert_eq!(st.cursor, mineral_protocol::PlayCursor::InQueue(0));
        assert_eq!(st.current_song, Some(song("a")));
        assert_eq!(st.queue_version, version.next().next());
        assert_eq!(st.queue_context, mineral_stats::QueueContext::Unknown);
        for id in ["c", "c2", "d", "d2"] {
            assert_eq!(
                st.context_overrides.get(&song(id).id.qualified()),
                Some(&mineral_stats::QueueContext::Manual)
            );
        }
    });
    core.set_play_mode(PlayMode::Shuffle, mineral_stats::Actor::User);
    assert!(core.queue_insert_next(
        vec![song("e"), song("e2"), song("e")],
        mineral_stats::QueueContext::Manual,
    ));
    assert!(core.queue_append(
        vec![song("f"), song("f2"), song("f")],
        mineral_stats::QueueContext::Manual,
    ));
    core.with_state(|st| {
        assert_eq!(
            st.queue.get(1..4),
            Some([song("e"), song("e2"), song("e")].as_slice())
        );
        assert_eq!(
            st.queue.get(st.queue.len() - 3..),
            Some([song("f"), song("f2"), song("f")].as_slice())
        );
        assert_eq!(st.cursor, mineral_protocol::PlayCursor::InQueue(0));
        assert_eq!(st.current_song, Some(song("a")));
        assert_eq!(
            st.original_queue,
            Some(vec![
                song("a"),
                song("e"),
                song("e2"),
                song("e"),
                song("c"),
                song("c2"),
                song("c"),
                song("b"),
                song("d"),
                song("d2"),
                song("d"),
                song("f"),
                song("f2"),
                song("f")
            ])
        );
    });
    core.set_play_mode(PlayMode::Sequential, mineral_stats::Actor::User);
    core.with_state(|st| {
        assert_eq!(
            st.queue.get(1..4),
            Some([song("e"), song("e2"), song("e")].as_slice())
        );
        assert_eq!(st.cursor, mineral_protocol::PlayCursor::InQueue(0));
        assert_eq!(st.current_song, Some(song("a")));
    });
    Ok(())
}

/// `stats.level` 热更会立即重配 recorder:覆盖为 off 时 A 不采集,
/// 撤销覆盖回 base(full)后 B 被记录。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_reapply_hot_swaps_stats_level() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(crate::config::DaemonConfig::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        liked_ids: None,
        playlists: None,
    })];
    let core = core_with_events_stats(
        channels,
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        /*script*/ None,
        recorder,
    )?;
    assert!(core.service_info().play_counts.enabled);
    let mut events_rx = core.notify().subscribe();
    // 覆盖 off → 播 A 被门掉。
    core.apply_config_overrides(vec![override_op(
        "stats.level",
        Some(mineral_protocol::BusValue::Str("off".to_owned())),
    )]);
    assert!(!service_update(&mut events_rx)?.play_counts.enabled);
    let gated = song("gated");
    core.play_song(
        &gated,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    core.spawn_on_played(gated.id.clone(), mineral_stats::FinishReason::Eof, 30_000);
    // 撤覆盖 → 回 base(full)→ 播 B 记。
    core.apply_config_overrides(vec![override_op("stats.level", None)]);
    let enabled = events_rx.try_recv()?;
    assert!(
        matches!(enabled, mineral_protocol::Event::ServiceInfoChanged { info } if info.play_counts.enabled)
    );
    let kept = song("kept");
    core.play_song(
        &kept,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    core.spawn_on_played(kept.id.clone(), mineral_stats::FinishReason::Eof, 60_000);
    // poll 到 B 落库(带超时)。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while store.totals(0..i64::MAX).await?.plays == 0 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:B 未落库");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // 只 B 被记(A 被 off 热更门掉)。
    assert_eq!(
        store.totals(0..i64::MAX).await?.plays,
        1,
        "off 门掉 A,只 B 记"
    );
    let opts = mineral_stats::ReportOptions::builder()
        .min_listen_ms(0)
        .top_limit(10)
        .build();
    let top = store
        .top_songs(0..i64::MAX, mineral_stats::TopBy::Plays, &opts)
        .await?;
    let first = top
        .first()
        .ok_or_else(|| color_eyre::eyre::eyre!("无 top 歌曲"))?;
    assert_eq!(first.song.value(), kept.id.value());
    Ok(())
}

/// set_config_base(配置文件重载入口)记一条 config_reloads(系统域,无 actor);
/// 此入口只由 mtime 重载回调驱动,一次调用 = 一次真重读文件。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_reload_records_system_event() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(crate::config::DaemonConfig::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        liked_ids: None,
        playlists: None,
    })];
    let core = core_with_events_stats(
        channels,
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        /*script*/ None,
        recorder,
    )?;
    // 模拟用户改文件后重载:默认树上改 audio.volume。
    let new_base = mineral_config::merge_tree(
        crate::config::default_daemon_tree()?,
        serde_json::json!({ "audio": { "volume": 42 } }),
    );
    core.set_config_base(new_base);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while store.status().await?.events == 0 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:config_reloads 未落库");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        store.status().await?.events,
        1,
        "一次重载记一条 config_reloads"
    );
    Ok(())
}

/// record_playlist_op:成功 Create + 失败多曲 AddSongs 各落一条 playlist_ops(行为域)。
/// 直接驱动接线方法,验证 op 名 / 错误映射真产数据。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playlist_op_records_to_stats_db() -> color_eyre::Result<()> {
    use mineral_model::PlaylistId;
    use mineral_task::{PlaylistWriteOp, WriteError};
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(crate::config::DaemonConfig::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        liked_ids: None,
        playlists: None,
    })];
    let core = core_with_events_stats(
        channels,
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        /*script*/ None,
        recorder,
    )?;
    core.record_playlist_op(
        &PlaylistWriteOp::Create {
            source: SourceKind::NETEASE,
            name: "mix".to_owned(),
        },
        None,
    );
    core.record_playlist_op(
        &PlaylistWriteOp::AddSongs {
            id: PlaylistId::new(SourceKind::NETEASE, "1"),
            songs: vec![song("a").id, song("b").id],
        },
        Some(&WriteError::RateLimited),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while store.status().await?.events < 2 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:playlist_ops 未落两条");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

/// record_search_result:Song 搜索记一条 searches;User 搜索(埋点不覆盖)被跳过。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_result_records_songs_skips_user_kind() -> color_eyre::Result<()> {
    use mineral_channel_core::Page;
    use mineral_model::SearchKind;
    use mineral_task::SearchPayload;
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(crate::config::DaemonConfig::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        liked_ids: None,
        playlists: None,
    })];
    let core = core_with_events_stats(
        channels,
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        /*script*/ None,
        recorder,
    )?;
    let page = Page::new(/*offset*/ 30, /*limit*/ 30); // 页码 1
    core.record_search_result(
        SourceKind::NETEASE,
        SearchKind::Song,
        "test",
        page,
        &SearchPayload::Songs(vec![song("a"), song("b")]),
    );
    // 用户搜索:埋点不覆盖,应被跳过(不落库)。
    core.record_search_result(
        SourceKind::NETEASE,
        SearchKind::User,
        "someone",
        page,
        &SearchPayload::Artists(Vec::new()),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let events = store.status().await?.events;
        if events >= 1 {
            assert_eq!(events, 1, "只 Song 搜索记录,User 搜索跳过");
            break;
        }
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:searches 未落库");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

//! play_song:取链失败 · stale 丢弃 · 本地/远端标记 · 下载后起播。

use super::*;
use pretty_assertions::assert_eq;

/// 端到端:play_song → play_started、spawn_on_played → play_ended,真 recorder 把一次
/// 播放写进 stats.db —— 证明埋点接线真产数据(非仅编译通过)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn play_records_to_stats_db() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        url_delay: None,
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
    let target = song("rec1");
    core.play_song(
        &target,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    ); // → play_started
    core.spawn_on_played(target.id.clone(), mineral_stats::FinishReason::Eof, 60_000); // → play_ended(eof)
    // actor 异步落库,poll 到出现(带超时兜底)。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while store.totals(0..i64::MAX).await?.plays == 0 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:play 未写进 stats.db");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let totals = store.totals(0..i64::MAX).await?;
    assert_eq!(totals.plays, 1);
    assert_eq!(totals.completed, 1, "completed=true → eof");
    assert_eq!(totals.listen_ms, 60_000);
    Ok(())
}

/// §4 per-song 覆盖:队列级 context 是 Playlist,插队一首带 Manual 覆盖的散曲;播放该散曲
/// 的行记 manual、播放队列曲的行记 playlist —— 插队曲不污染歌单归属。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn insert_next_override_does_not_pollute_queue_context() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        url_delay: None,
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
    let queued = song("q1");
    let playlist = mineral_model::PlaylistId::new(SourceKind::NETEASE, "pl");
    core.set_queue(
        vec![queued.clone()],
        &queued.id,
        mineral_stats::QueueContext::Playlist {
            id: playlist.clone(),
            name: None,
        },
    );
    // 队列曲:继承队列级 Playlist context。
    core.play_song(
        &queued,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    core.spawn_on_played(queued.id.clone(), mineral_stats::FinishReason::Eof, 60_000);
    // 插队散曲带 Manual 覆盖:播它的行应记 manual 而非 playlist。
    let inserted = song("ins");
    core.queue_insert_next(inserted.clone(), mineral_stats::QueueContext::Manual);
    core.play_song(
        &inserted,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    core.spawn_on_played(
        inserted.id.clone(),
        mineral_stats::FinishReason::Eof,
        30_000,
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while store.totals(0..i64::MAX).await?.plays < 2 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:两次播放未写进 stats.db");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let contexts = store.top_contexts(0..i64::MAX, None, 0, 10).await?;
    let playlist_slice = contexts
        .iter()
        .find(|c| c.kind == "playlist")
        .ok_or_else(|| color_eyre::eyre::eyre!("应有 playlist 语境:{contexts:?}"))?;
    assert_eq!(playlist_slice.plays, 1, "只队列曲计入歌单,插队曲不污染");
    assert_eq!(playlist_slice.reference.as_deref(), Some("netease:pl"));
    let manual_slice = contexts
        .iter()
        .find(|c| c.kind == "manual")
        .ok_or_else(|| color_eyre::eyre::eyre!("插队曲应记 manual 语境:{contexts:?}"))?;
    assert_eq!(manual_slice.plays, 1, "插队散曲归 manual");
    Ok(())
}

/// 直接改播(client PlaySong / 脚本 Play 的规矩:settle_interrupted + play_song):被
/// 打断的在播曲结算成 skip 行落库,不因 pending 被新起播覆盖而丢失。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_play_settles_interrupted_song() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        url_delay: None,
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
    let (a, b) = (song("a"), song("b"));
    core.set_queue(
        vec![a.clone(), b.clone()],
        &a.id,
        mineral_stats::QueueContext::Unknown,
    );
    core.play_song(
        &a,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    // 直接改播 b:先结算被打断的 a(skip),再起播——client PlaySong handler 的顺序。
    core.settle_interrupted();
    core.play_song(
        &b,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    core.spawn_on_played(b.id.clone(), mineral_stats::FinishReason::Eof, 60_000);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while store.totals(0..i64::MAX).await?.plays < 2 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:被打断曲的 play 行未落库");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let totals = store.totals(0..i64::MAX).await?;
    assert_eq!(totals.plays, 2, "被打断的 a 也落行,不丢");
    assert_eq!(totals.completed, 1, "只有 b 是 eof,a 记 skip");
    Ok(())
}

/// per-song 语境覆盖的生命周期:取用即消费(同 id 二次起播回落队列语境);set_queue 换
/// 队列清空全部覆盖(未播插队曲的陈旧覆盖不得泄漏到新队列的同 id 起播)。
#[tokio::test(flavor = "multi_thread")]
async fn context_override_consumed_once_and_cleared_on_set_queue() -> color_eyre::Result<()> {
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with(calls)?;
    let playlist_context = || mineral_stats::QueueContext::Playlist {
        id: mineral_model::PlaylistId::new(SourceKind::NETEASE, "pl"),
        name: None,
    };
    core.set_queue(vec![song("a")], &song("a").id, playlist_context());
    // 插队散曲:首次取用得覆盖,且取用即消费——再取回落队列语境。
    core.queue_insert_next(song("x"), mineral_stats::QueueContext::Manual);
    assert!(
        matches!(
            core.take_play_context(&song("x").id),
            mineral_stats::QueueContext::Manual
        ),
        "首次取用命中覆盖"
    );
    assert!(
        matches!(
            core.take_play_context(&song("x").id),
            mineral_stats::QueueContext::Playlist { .. }
        ),
        "覆盖取用即消费,二次取回落队列语境"
    );
    // 未播就换队列:旧覆盖清空,同 id 在新队列起播不得继承陈旧 Manual。
    core.queue_insert_next(song("y"), mineral_stats::QueueContext::Manual);
    core.set_queue(vec![song("y")], &song("y").id, playlist_context());
    assert!(
        matches!(
            core.take_play_context(&song("y").id),
            mineral_stats::QueueContext::Playlist { .. }
        ),
        "set_queue 清空旧覆盖,不泄漏陈旧语境"
    );
    Ok(())
}

/// SongUrl 取链失败 → `SongUrlFailed` 事件 → unplayable 口(无脚本)推
/// `TrackFinished{reason: Error}`(RecordingChannel 的 `song_urls` 恒 `Err`)。
/// 事件由 tick 的 drain 消化,测试里手动泵 `consume_events_once`。
#[tokio::test(flavor = "multi_thread")]
async fn play_song_url_failure_notifies_error() -> color_eyre::Result<()> {
    use mineral_protocol::{Event, FinishReason};
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 8);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls,
        url_delay: None,
        liked_ids: None,
        playlists: None,
    })];
    let core = core_with_events(
        channels,
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        events_tx,
        /*script*/ None,
    )?;
    let target = song("e1");
    core.play_song(
        &target,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        core.consume_events_once();
        match events_rx.try_recv() {
            Ok(Event::TrackFinished { song_id, reason }) => {
                assert_eq!(song_id, target.id);
                assert_eq!(reason, FinishReason::Error);
                return Ok(());
            }
            Ok(_other) => {}
            Err(_empty) => {
                if std::time::Instant::now() > deadline {
                    color_eyre::eyre::bail!("超时未收到 TrackFinished(Error)");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

/// 取链失败但用户已切走(失败的不是当前曲)→ 不报 Error(防迟到误报)。
#[tokio::test(flavor = "multi_thread")]
async fn stale_url_failure_does_not_notify() -> color_eyre::Result<()> {
    use mineral_protocol::Event;
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 8);
    // 失败前人为延迟:保证「切走」必然发生在任务失败之前,时序确定不 flaky。
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls,
        url_delay: Some(Duration::from_millis(200)),
        liked_ids: None,
        playlists: None,
    })];
    let core = core_with_events(
        channels,
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        events_tx,
        /*script*/ None,
    )?;
    core.play_song(
        &song("e1"),
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    // 立即切走:当前曲不再是 e1,e1 的失败(SongUrlFailed 分流落 Drop)不该报。
    core.with_state(|st| st.current_song = Some(song("e2")));
    tokio::time::sleep(Duration::from_millis(500)).await;
    core.consume_events_once();
    while let Ok(event) = events_rx.try_recv() {
        assert!(
            !matches!(event, Event::TrackFinished { ref song_id, .. } if song_id == &song("e1").id),
            "已切走的失败不该报 TrackFinished,实得 {event:?}"
        );
    }
    Ok(())
}

/// 取链失败经 handle_song_url_failed 落一条 url_resolutions(Error);真 recorder 写库,
/// 证 events.rs 接线产数据(非仅编译)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn url_resolution_error_records_to_stats_db() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        url_delay: None,
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
    core.play_song(
        &song("e1"),
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    ); // song_urls 恒 Err → SongUrlFailed
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        core.consume_events_once();
        if store.status().await?.events >= 1 {
            break;
        }
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:url_resolutions 未落库");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}

/// 一次真 Lyrics 取数经 scheduler 收束 → 发 FetchDone → 记一条 fetches;真 recorder 落库,
/// 证 mineral-task lane 埋点信号 + events.rs 接线全链产数据。Lyrics 不触发其他 stats 事件,
/// 故 events 恰为 1(隔离 fetch)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lyrics_fetch_records_to_stats_db() -> color_eyre::Result<()> {
    use mineral_task::{ChannelFetchKind, Priority, TaskKind};
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        url_delay: None,
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
    core.submit_task(
        TaskKind::ChannelFetch(ChannelFetchKind::Lyrics {
            song_id: song("a").id,
        }),
        Priority::User,
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        core.consume_events_once();
        if store.status().await?.events >= 1 {
            break;
        }
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:fetches 未落库");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}

/// play_song(手动切歌)应清掉过期的 gapless 预排(`queued`),防止跨切歌泄漏预排状态。
#[tokio::test]
async fn play_song_clears_stale_queued() -> color_eyre::Result<()> {
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with(calls)?;
    {
        let mut st = core.inner.state.lock();
        st.queue = vec![song("a"), song("b")];
        st.cursor = mineral_protocol::PlayCursor::InQueue(0);
        st.current_song = Some(song("a"));
        st.queued = Some(crate::gapless::Queued {
            song: song("b"),
            play_url: None,
            origin: PlaybackOrigin::Remote,
            capturing: None,
        });
    }
    core.play_song(
        &song("a"),
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    assert!(
        core.inner.state.lock().queued.is_none(),
        "手动切歌应清掉过期预排"
    );
    Ok(())
}

/// play_song 无本地副本(media_cache disabled + music_dir None)→ 走远端,
/// snapshot.play_origin == Remote(验证 play_song → State → snapshot 接线)。
#[tokio::test]
async fn play_song_without_local_marks_remote() -> color_eyre::Result<()> {
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with(calls)?;
    core.play_song(
        &song("a"),
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );
    assert_eq!(
        core.sync(PlayerVersions::default()).play_origin,
        Some(PlaybackOrigin::Remote),
        "无本地副本应标记为远端"
    );
    Ok(())
}

/// 一首带专辑的测试歌曲(库路径取 album/title)。
fn song_with_album(id: &str, name: &str, album: &str) -> Song {
    Song::builder()
        .id(SongId::new(SourceKind::NETEASE, id))
        .name(name.to_owned())
        .album(Some(AlbumRef {
            id: AlbumId::new(SourceKind::NETEASE, "0"),
            name: album.to_owned(),
        }))
        .duration_ms(Some(1000))
        .build()
}

/// 端到端:**真下载**一首(走进程内 HTTP server)→ **再播放** → 应解析到刚下载的文件
/// (`origin=Download` / `quality=Lossless`,零网络、不进缓存)。
///
/// 这是「下载的歌就该从下载库播」这条业务规则的端到端守卫:跨 download → resolve →
/// State → snapshot 全链路。若下载又顺手填了缓存,play_song 会命中缓存副本(`origin=Cache`)
/// → 此测试变红。
// multi_thread:走真实 TCP I/O(serve_once + reqwest)且起 audio engine,二者在单线程
// runtime 下都脆(协作调度 / engine 需多线程),重负载时 flaky;给独立 worker 线程。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_then_plays_from_download() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let media_cache = MediaCache::open(&persist, dir.path().join("cache"), 1_000_000_000).await?;
    let music_dir = dir.path().join("music");
    let s = song_with_album("1", "捕风", "野泳");

    // 1. 真下载到 music_dir(走进程内 HTTP server)。
    let url = serve_once(b"FAKEFLACDATA".to_vec()).await?;
    let dl_channel = UrlChannel { url };
    let http = reqwest::Client::new();
    let progress = Arc::new(Mutex::new(DownloadProgress::default()));
    download_song(
        &dl_channel,
        &crate::download::DownloadEnv {
            http: &http,
            music_dir: &music_dir,
            hooks: &crate::hook_bridge::HookGate::disabled(),
        },
        &s,
        BitRate::Lossless,
        &progress,
        /*speed_tick*/ Duration::from_millis(150),
    )
    .await?;

    // 2. 用同一 music_dir + media_cache 起 core,播放。
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls,
        url_delay: None,
        liked_ids: None,
        playlists: None,
    })];
    let core = core_with_channels(channels, persist, Some(music_dir), media_cache)?;
    core.play_song(
        &s,
        mineral_stats::PlayOrigin::Explicit,
        mineral_stats::Actor::User,
    );

    // 3. 应解析到刚下载的 lossless。
    let sync = core.sync(PlayerVersions::default());
    assert_eq!(
        sync.play_origin,
        Some(PlaybackOrigin::Download),
        "下载的歌应从下载库播放,而非缓存 / 网络"
    );
    let pu = sync
        .current
        .ok_or_else(|| color_eyre::eyre::eyre!("known=0 应返回 current 重段"))?
        .play_url
        .ok_or_else(|| color_eyre::eyre::eyre!("本地命中应填 play_url"))?;
    assert_eq!(pu.quality, BitRate::Lossless, "命中音质应为 lossless");
    Ok(())
}

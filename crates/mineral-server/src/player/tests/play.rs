//! play_song:取链失败 · stale 丢弃 · 本地/远端标记 · 下载后起播。

use super::*;
use pretty_assertions::assert_eq;

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
    core.play_song(&target);
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
    core.play_song(&song("e1"));
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

/// play_song(手动切歌)应清掉过期的 gapless 预排(`queued`),防止跨切歌泄漏预排状态。
#[tokio::test]
async fn play_song_clears_stale_queued() -> color_eyre::Result<()> {
    let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
    let core = core_with(calls)?;
    {
        let mut st = core.inner.state.lock();
        st.queue = vec![song("a"), song("b")];
        st.queue_sel = 0;
        st.current_song = Some(song("a"));
        st.queued = Some(crate::gapless::Queued {
            song: song("b"),
            play_url: None,
            origin: PlaybackOrigin::Remote,
            capturing: None,
        });
    }
    core.play_song(&song("a"));
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
    core.play_song(&song("a"));
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
    core.play_song(&s);

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

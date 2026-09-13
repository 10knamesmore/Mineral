//! Scheduler 端到端单测:不依赖真实 channel,本地 fake。

use std::sync::Arc;

use async_trait::async_trait;
use mineral_channel_core::{ChannelCaps, Error, MusicChannel, Page, Result, SearchHits};
use mineral_model::{
    Album, AlbumId, Lyrics, Playlist, PlaylistId, SearchKind, Song, SongId, SourceKind,
};
use mineral_task::{
    ChannelFetchKind, ChannelFetchKindTag, Priority, Scheduler, SearchPayload, TaskEvent, TaskKind,
    TaskOutcome,
};
use tokio::sync::Semaphore;

/// fake channel:歌单、歌词与歌曲搜索成功且可被 gate 阻塞，其他端点返回 NotSupported。
///
/// 用 Semaphore 而非 Notify 当 gate ——`add_permits` 即使在没人 await 时调用,
/// 之后的 `acquire` 也能立刻拿到,避免测试里的"先 notify 再 await"竞态。
struct FakeChannel {
    /// 当前用户的歌单。
    playlists: Vec<Playlist>,

    /// 每个 permit 放行一次成功端点调用。
    gate: Option<Arc<Semaphore>>,

    /// 每次进入歌曲搜索端点时提供一个 permit，供测试确定取消时机。
    search_started: Arc<Semaphore>,
}

impl FakeChannel {
    fn new(gate: Option<Arc<Semaphore>>) -> Self {
        let pl = Playlist::builder()
            .id(PlaylistId::new(SourceKind::NETEASE, "p1"))
            .name(String::from("P1"))
            .build();
        Self {
            playlists: vec![pl],
            gate,
            search_started: Arc::new(Semaphore::new(0)),
        }
    }

    async fn maybe_wait(&self) {
        if let Some(g) = &self.gate {
            // forget 让 permit 不归还(每个 release N 个 permit 只能放行 N 个 await)
            if let Ok(p) = g.acquire().await {
                p.forget();
            }
        }
    }
}

#[async_trait]
impl MusicChannel for FakeChannel {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(vec![SearchKind::Song])
            .playlist_edit(false)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(&self, _q: &str, _p: Page) -> Result<SearchHits<Song>> {
        self.search_started.add_permits(1);
        self.maybe_wait().await;
        Ok(SearchHits::new(Vec::new(), false))
    }
    async fn search_albums(&self, _q: &str, _p: Page) -> Result<SearchHits<Album>> {
        Err(Error::NotSupported)
    }
    async fn search_playlists(&self, _q: &str, _p: Page) -> Result<SearchHits<Playlist>> {
        Err(Error::NotSupported)
    }
    async fn songs_detail(&self, _ids: &[SongId]) -> Result<Vec<Song>> {
        Err(Error::NotSupported)
    }
    async fn album_detail(&self, _id: &AlbumId) -> Result<Album> {
        Err(Error::NotSupported)
    }
    async fn playlist_detail(&self, id: &PlaylistId) -> Result<Playlist> {
        self.maybe_wait().await;
        Ok(Playlist::builder()
            .id(id.clone())
            .name(String::new())
            .build())
    }
    async fn lyrics(&self, _id: &SongId) -> Result<Lyrics> {
        self.maybe_wait().await;
        Ok(Lyrics {
            lines: mineral_model::parse_lrc("[00:01.00]hello\n[00:02.50]world"),
        })
    }
    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        self.maybe_wait().await;
        Ok(self.playlists.clone())
    }
}

fn my_playlists_kind() -> TaskKind {
    TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists {
        source: SourceKind::NETEASE,
    })
}

fn playlist_tracks_kind() -> TaskKind {
    TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail {
        id: PlaylistId::new(SourceKind::NETEASE, "p1"),
    })
}

fn lyrics_kind(song: &str) -> TaskKind {
    TaskKind::ChannelFetch(ChannelFetchKind::Lyrics {
        song_id: SongId::new(SourceKind::NETEASE, song),
    })
}

fn channels(gate: Option<Arc<Semaphore>>) -> Vec<Arc<dyn MusicChannel>> {
    let ch: Arc<dyn MusicChannel> = Arc::new(FakeChannel::new(gate));
    vec![ch]
}

#[tokio::test]
async fn submit_and_done_ok() -> color_eyre::Result<()> {
    let sched = Scheduler::new(&channels(None), /*workers_per_channel*/ 8);
    let h = sched.submit(my_playlists_kind(), Priority::User);
    assert_eq!(h.done().await, TaskOutcome::Ok);

    // 结果事件(PlaylistsFetched)+ 每次取数收束的 FetchDone 埋点信号。
    let evs = sched.drain_events();
    assert_eq!(evs.len(), 2, "结果事件 + FetchDone");
    assert!(
        evs.iter()
            .any(|e| matches!(e, TaskEvent::PlaylistsFetched { .. })),
        "应含结果事件"
    );
    assert!(
        evs.iter().any(|e| matches!(
            e,
            TaskEvent::FetchDone {
                outcome: TaskOutcome::Ok,
                ..
            }
        )),
        "应含 FetchDone(Ok) 埋点信号"
    );
    Ok(())
}

#[tokio::test]
async fn dedup_returns_same_handle() -> color_eyre::Result<()> {
    let gate = Arc::new(Semaphore::new(0));
    let sched = Scheduler::new(
        &channels(Some(Arc::clone(&gate))),
        /*workers_per_channel*/ 8,
    );
    let h1 = sched.submit(my_playlists_kind(), Priority::User);
    let h2 = sched.submit(my_playlists_kind(), Priority::User);

    gate.add_permits(1);
    assert_eq!(h1.done().await, TaskOutcome::Ok);
    assert_eq!(h2.done().await, TaskOutcome::Ok);
    Ok(())
}

#[tokio::test]
async fn cancel_where_batch() -> color_eyre::Result<()> {
    let gate = Arc::new(Semaphore::new(0));
    let sched = Scheduler::new(&channels(Some(gate)), /*workers_per_channel*/ 8);
    let h1 = sched.submit(my_playlists_kind(), Priority::User);
    let h2 = sched.submit(playlist_tracks_kind(), Priority::User);
    sched.cancel_where(|k| matches!(k, TaskKind::ChannelFetch(_)));
    assert_eq!(h1.done().await, TaskOutcome::Cancelled);
    assert_eq!(h2.done().await, TaskOutcome::Cancelled);
    Ok(())
}

#[tokio::test]
async fn escalate_replaces_background() -> color_eyre::Result<()> {
    let gate = Arc::new(Semaphore::new(0));
    let sched = Scheduler::new(
        &channels(Some(Arc::clone(&gate))),
        /*workers_per_channel*/ 8,
    );
    let h_bg = sched.submit(my_playlists_kind(), Priority::Background);
    let h_user = sched.submit(my_playlists_kind(), Priority::User);
    assert_eq!(h_bg.done().await, TaskOutcome::Cancelled);

    gate.add_permits(1);
    assert_eq!(h_user.done().await, TaskOutcome::Ok);
    Ok(())
}

#[tokio::test]
async fn lyrics_emits_event() -> color_eyre::Result<()> {
    let sched = Scheduler::new(&channels(None), /*workers_per_channel*/ 8);
    let h = sched.submit(lyrics_kind("s3"), Priority::User);
    assert_eq!(h.done().await, TaskOutcome::Ok);

    let evs = sched.drain_events();
    let found = evs.iter().any(|e| {
        matches!(
            e,
            TaskEvent::LyricsReady { song_id, lyrics }
                if song_id.as_str() == "s3"
                    && lyrics
                        .lines
                        .iter()
                        .any(|l| l.kind.text().contains("hello"))
        )
    });
    assert!(found, "expected LyricsReady, got {evs:?}");
    Ok(())
}

/// 搜索端点失败时回带完整请求，且保留失败埋点。
#[tokio::test]
async fn search_failure_emits_request_and_fetch_done() -> color_eyre::Result<()> {
    let sched = Scheduler::new(&channels(None), /*workers_per_channel*/ 8);
    let source = SourceKind::NETEASE;
    let kind = SearchKind::Album;
    let query = "续页失败".to_owned();
    let page = Page::new(60, 20);
    let h = sched.submit(
        TaskKind::ChannelFetch(ChannelFetchKind::Search {
            source,
            kind,
            query: query.clone(),
            page,
        }),
        Priority::User,
    );
    assert_eq!(h.done().await, TaskOutcome::Failed);

    let events = sched.drain_events();
    assert_eq!(events.len(), 2, "失败事件 + FetchDone: {events:?}");
    assert!(events.contains(&TaskEvent::SearchPageFailed {
        source,
        kind,
        query,
        page,
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        TaskEvent::FetchDone {
            kind: ChannelFetchKindTag::Search,
            source: event_source,
            target_ref: None,
            from_user: true,
            outcome: TaskOutcome::Failed,
            ..
        } if *event_source == source
    )));
    Ok(())
}

/// 排队期间和端点执行期间取消搜索，都会回带请求并保留取消埋点。
#[tokio::test]
async fn search_cancellation_emits_request_before_and_during_execution() -> color_eyre::Result<()> {
    for cancel_while_running in [false, true] {
        let channel = FakeChannel::new(Some(Arc::new(Semaphore::new(0))));
        let search_started = Arc::clone(&channel.search_started);
        let channel: Arc<dyn MusicChannel> = Arc::new(channel);
        let sched = Scheduler::new(&[channel], /*workers_per_channel*/ 1);
        let source = SourceKind::NETEASE;
        let kind = SearchKind::Song;
        let query = "取消续页".to_owned();
        let page = Page::new(90, 15);
        let h = sched.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::Search {
                source,
                kind,
                query: query.clone(),
                page,
            }),
            Priority::User,
        );
        if cancel_while_running {
            tokio::time::timeout(std::time::Duration::from_secs(5), search_started.acquire())
                .await??
                .forget();
        }
        // 当前线程 runtime 在首次 await 前不运行 worker，覆盖排队时已取消的分支。
        sched.cancel_where(|task| {
            matches!(
                task,
                TaskKind::ChannelFetch(ChannelFetchKind::Search { .. })
            )
        });
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), h.done()).await?,
            TaskOutcome::Cancelled
        );

        let events = sched.drain_events();
        assert_eq!(events.len(), 2, "取消事件 + FetchDone: {events:?}");
        assert!(events.contains(&TaskEvent::SearchPageFailed {
            source,
            kind,
            query,
            page,
        }));
        assert!(events.iter().any(|event| matches!(
            event,
            TaskEvent::FetchDone {
                kind: ChannelFetchKindTag::Search,
                source: event_source,
                target_ref: None,
                from_user: true,
                outcome: TaskOutcome::Cancelled,
                ..
            } if *event_source == source
        )));
        assert_eq!(search_started.available_permits(), 0);
    }
    Ok(())
}

/// 成功搜索只发送结果与成功埋点，不能误发释放待收取页的失败事件。
#[tokio::test]
async fn successful_search_emits_results_without_failure() -> color_eyre::Result<()> {
    let sched = Scheduler::new(&channels(None), /*workers_per_channel*/ 8);
    let source = SourceKind::NETEASE;
    let kind = SearchKind::Song;
    let query = "成功续页".to_owned();
    let page = Page::new(40, 10);
    let h = sched.submit(
        TaskKind::ChannelFetch(ChannelFetchKind::Search {
            source,
            kind,
            query: query.clone(),
            page,
        }),
        Priority::User,
    );
    assert_eq!(h.done().await, TaskOutcome::Ok);

    let events = sched.drain_events();
    assert_eq!(events.len(), 2, "搜索结果 + FetchDone: {events:?}");
    assert!(events.contains(&TaskEvent::SearchResults {
        source,
        kind,
        query,
        page,
        payload: SearchPayload::Songs(Vec::new()),
        has_more: Some(false),
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        TaskEvent::FetchDone {
            kind: ChannelFetchKindTag::Search,
            source: event_source,
            target_ref: None,
            from_user: true,
            outcome: TaskOutcome::Ok,
            ..
        } if *event_source == source
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, TaskEvent::SearchPageFailed { .. }))
    );
    Ok(())
}

// ---------------- PlaylistWrite lane ----------------

/// 记录写调用时序的桩 channel:每次 rename 记 start/end 各一条,中间 sleep
/// 放大并发窗口——若 lane 不是串行,start/end 必然交错。
struct WriteRecorder {
    /// 时序记录(`start:<name>` / `end:<name>`)。
    events: Arc<parking_lot::Mutex<Vec<String>>>,
}

#[async_trait]
impl MusicChannel for WriteRecorder {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    fn caps(&self) -> mineral_channel_core::ChannelCaps {
        mineral_channel_core::ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(true)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(&self, _q: &str, _p: Page) -> Result<SearchHits<Song>> {
        Err(Error::NotSupported)
    }
    async fn search_albums(&self, _q: &str, _p: Page) -> Result<SearchHits<Album>> {
        Err(Error::NotSupported)
    }
    async fn search_playlists(&self, _q: &str, _p: Page) -> Result<SearchHits<Playlist>> {
        Err(Error::NotSupported)
    }
    async fn songs_detail(&self, _ids: &[SongId]) -> Result<Vec<Song>> {
        Err(Error::NotSupported)
    }
    async fn album_detail(&self, _id: &mineral_model::AlbumId) -> Result<mineral_model::Album> {
        Err(Error::NotSupported)
    }
    async fn playlist_detail(&self, _id: &PlaylistId) -> Result<Playlist> {
        Err(Error::NotSupported)
    }
    async fn lyrics(&self, _id: &SongId) -> Result<Lyrics> {
        Err(Error::NotSupported)
    }

    async fn rename_playlist(&self, _id: &PlaylistId, name: &str) -> Result<()> {
        self.events.lock().push(format!("start:{name}"));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        self.events.lock().push(format!("end:{name}"));
        Ok(())
    }
}

fn rename_op(name: &str) -> TaskKind {
    TaskKind::PlaylistWrite(mineral_task::PlaylistWriteOp::Rename {
        id: PlaylistId::new(SourceKind::NETEASE, "p1"),
        name: name.to_owned(),
    })
}

/// 同源写操作严格串行且保持提交顺序:start/end 成对相邻,绝不交错。
#[tokio::test]
async fn playlist_writes_run_serially_in_submit_order() -> color_eyre::Result<()> {
    let events = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let ch: Arc<dyn MusicChannel> = Arc::new(WriteRecorder {
        events: Arc::clone(&events),
    });
    let sched = Scheduler::new(&[ch], /*workers_per_channel*/ 8);

    let h1 = sched.submit(rename_op("a"), Priority::User);
    let h2 = sched.submit(rename_op("b"), Priority::User);
    let h3 = sched.submit(rename_op("c"), Priority::User);
    assert_eq!(h1.done().await, TaskOutcome::Ok);
    assert_eq!(h2.done().await, TaskOutcome::Ok);
    assert_eq!(h3.done().await, TaskOutcome::Ok);

    assert_eq!(
        *events.lock(),
        vec!["start:a", "end:a", "start:b", "end:b", "start:c", "end:c"]
    );

    let evs = sched.drain_events();
    let done_ok = evs
        .iter()
        .filter(|e| matches!(e, TaskEvent::PlaylistWriteDone { error: None, .. }))
        .count();
    assert_eq!(done_ok, 3);
    Ok(())
}

/// 写操作失败时发送结构化错误事件（默认 trait 实现返回 NotSupported）。
#[tokio::test]
async fn playlist_write_failure_emits_error_event() -> color_eyre::Result<()> {
    // FakeChannel 没实现写方法 → trait 默认 NotSupported
    let sched = Scheduler::new(&channels(None), /*workers_per_channel*/ 8);
    let h = sched.submit(rename_op("x"), Priority::User);
    assert_eq!(h.done().await, TaskOutcome::Failed);

    let evs = sched.drain_events();
    let found = evs.iter().any(|e| {
        matches!(
            e,
            TaskEvent::PlaylistWriteDone {
                error: Some(mineral_task::WriteError::NotSupported),
                ..
            }
        )
    });
    assert!(
        found,
        "expected PlaylistWriteDone(NotSupported), got {evs:?}"
    );
    Ok(())
}

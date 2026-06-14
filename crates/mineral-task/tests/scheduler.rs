//! Scheduler 端到端单测:不依赖真实 channel,本地 fake。

use std::sync::Arc;

use async_trait::async_trait;
use mineral_channel_core::{ChannelCaps, Error, MusicChannel, Page, Result};
use mineral_model::{
    Album, AlbumId, BitRate, Lyrics, MediaUrl, PlayUrl, Playlist, PlaylistId, Song, SongId,
    SourceKind,
};
use mineral_task::{ChannelFetchKind, Priority, Scheduler, TaskEvent, TaskKind, TaskOutcome};
use tokio::sync::Semaphore;

/// fake channel:my_playlists/playlist_detail 可被 gate 阻塞,其他端点全部 NotSupported。
///
/// 用 Semaphore 而非 Notify 当 gate ——`add_permits` 即使在没人 await 时调用,
/// 之后的 `acquire` 也能立刻拿到,避免测试里的"先 notify 再 await"竞态。
struct FakeChannel {
    playlists: Vec<Playlist>,
    gate: Option<Arc<Semaphore>>,
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
            .searchable(Vec::new())
            .playlist_edit(false)
            .build()
    }

    async fn search_songs(&self, _q: &str, _p: Page) -> Result<Vec<Song>> {
        Err(Error::NotSupported)
    }
    async fn search_albums(&self, _q: &str, _p: Page) -> Result<Vec<Album>> {
        Err(Error::NotSupported)
    }
    async fn search_playlists(&self, _q: &str, _p: Page) -> Result<Vec<Playlist>> {
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
    async fn song_urls(&self, ids: &[SongId], _q: BitRate) -> Result<Vec<PlayUrl>> {
        self.maybe_wait().await;
        let id = ids
            .first()
            .cloned()
            .ok_or(Error::Other(color_eyre::eyre::eyre!("empty ids")))?;
        Ok(vec![PlayUrl {
            song_id: id,
            url: MediaUrl::remote("https://example.com/a.mp3")
                .map_err(|e| Error::Other(color_eyre::eyre::eyre!("{e}")))?,
            bitrate_bps: 320_000,
            quality: BitRate::Higher,
            size: 0,
            format: mineral_model::AudioFormat::Mp3,
            bit_depth: None,
        }])
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

fn song_url_kind(song: &str) -> TaskKind {
    TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
        song_id: SongId::new(SourceKind::NETEASE, song),
        quality: BitRate::Higher,
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

    let evs = sched.drain_events();
    assert_eq!(evs.len(), 1);
    assert!(matches!(
        evs.first(),
        Some(TaskEvent::PlaylistsFetched { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn cancel_yields_cancelled() -> color_eyre::Result<()> {
    let gate = Arc::new(Semaphore::new(0));
    let sched = Scheduler::new(&channels(Some(gate)), /*workers_per_channel*/ 8);
    let h = sched.submit(my_playlists_kind(), Priority::User);
    h.cancel();
    assert_eq!(h.done().await, TaskOutcome::Cancelled);
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
    assert_eq!(h1.id, h2.id);

    gate.add_permits(1);
    assert_eq!(h1.done().await, TaskOutcome::Ok);
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
    assert_ne!(h_bg.id, h_user.id);
    assert_eq!(h_bg.done().await, TaskOutcome::Cancelled);

    gate.add_permits(1);
    assert_eq!(h_user.done().await, TaskOutcome::Ok);
    Ok(())
}

#[tokio::test]
async fn song_url_emits_play_url_ready() -> color_eyre::Result<()> {
    let sched = Scheduler::new(&channels(None), /*workers_per_channel*/ 8);
    let h = sched.submit(song_url_kind("s1"), Priority::User);
    assert_eq!(h.done().await, TaskOutcome::Ok);

    let evs = sched.drain_events();
    let found = evs.iter().any(|e| {
        matches!(
            e,
            TaskEvent::PlayUrlReady { song_id, .. } if song_id.as_str() == "s1"
        )
    });
    assert!(found, "expected PlayUrlReady, got {evs:?}");
    Ok(())
}

#[tokio::test]
async fn song_url_dedup_returns_same_handle() -> color_eyre::Result<()> {
    let gate = Arc::new(Semaphore::new(0));
    let sched = Scheduler::new(
        &channels(Some(Arc::clone(&gate))),
        /*workers_per_channel*/ 8,
    );
    let h1 = sched.submit(song_url_kind("s2"), Priority::User);
    let h2 = sched.submit(song_url_kind("s2"), Priority::User);
    assert_eq!(h1.id, h2.id);

    gate.add_permits(1);
    assert_eq!(h1.done().await, TaskOutcome::Ok);
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
            .build()
    }

    async fn search_songs(&self, _q: &str, _p: Page) -> Result<Vec<Song>> {
        Err(Error::NotSupported)
    }
    async fn search_albums(&self, _q: &str, _p: Page) -> Result<Vec<Album>> {
        Err(Error::NotSupported)
    }
    async fn search_playlists(&self, _q: &str, _p: Page) -> Result<Vec<Playlist>> {
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
    async fn song_urls(&self, _ids: &[SongId], _q: BitRate) -> Result<Vec<PlayUrl>> {
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

/// 写操作失败也发事件(与 ChannelFetch"失败只留日志"刻意不同),
/// 且错误结构化(默认 trait 实现 → NotSupported)。
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

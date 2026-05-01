//! Scheduler 端到端单测:不依赖真实 channel,本地 fake。

use std::sync::Arc;

use async_trait::async_trait;
use mineral_channel_core::{Error, MusicChannel, Page, Result};
use mineral_model::{
    Album, AlbumId, BitRate, Lyrics, MediaUrl, PlayUrl, Playlist, PlaylistId, Song, SongId,
    SourceKind,
};
use mineral_task::{ChannelFetchKind, Priority, Scheduler, TaskEvent, TaskKind, TaskOutcome};
use tokio::sync::Semaphore;

/// fake channel:my_playlists/songs_in_playlist 可被 gate 阻塞,其他端点全部 NotSupported。
///
/// 用 Semaphore 而非 Notify 当 gate ——`add_permits` 即使在没人 await 时调用,
/// 之后的 `acquire` 也能立刻拿到,避免测试里的"先 notify 再 await"竞态。
struct FakeChannel {
    playlists: Vec<Playlist>,
    gate: Option<Arc<Semaphore>>,
}

impl FakeChannel {
    fn new(gate: Option<Arc<Semaphore>>) -> Self {
        let pl = Playlist {
            source: SourceKind::Netease,
            id: PlaylistId::new("p1"),
            name: String::from("P1"),
            description: String::new(),
            cover_url: None,
            track_count: 0,
            songs: Vec::new(),
        };
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
        SourceKind::Netease
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
    async fn songs_in_album(&self, _id: &AlbumId) -> Result<Vec<Song>> {
        Err(Error::NotSupported)
    }
    async fn songs_in_playlist(&self, _id: &PlaylistId) -> Result<Vec<Song>> {
        self.maybe_wait().await;
        Ok(Vec::new())
    }
    async fn song_urls(&self, ids: &[SongId], _q: BitRate) -> Result<Vec<PlayUrl>> {
        self.maybe_wait().await;
        let id = ids
            .first()
            .cloned()
            .ok_or(Error::Other(color_eyre::eyre::eyre!("empty ids")))?;
        Ok(vec![PlayUrl {
            source: SourceKind::Netease,
            song_id: id,
            url: MediaUrl::remote("https://example.com/a.mp3")
                .map_err(|e| Error::Other(color_eyre::eyre::eyre!("{e}")))?,
            bitrate_bps: 320_000,
            quality: BitRate::Higher,
            size: 0,
            format: String::from("mp3"),
        }])
    }
    async fn lyrics(&self, _id: &SongId) -> Result<Lyrics> {
        Err(Error::NotSupported)
    }
    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        self.maybe_wait().await;
        Ok(self.playlists.clone())
    }
}

fn my_playlists_kind() -> TaskKind {
    TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists {
        source: SourceKind::Netease,
    })
}

fn playlist_tracks_kind() -> TaskKind {
    TaskKind::ChannelFetch(ChannelFetchKind::PlaylistTracks {
        source: SourceKind::Netease,
        id: PlaylistId::new("p1"),
    })
}

fn song_url_kind(song: &str) -> TaskKind {
    TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
        source: SourceKind::Netease,
        song_id: SongId::new(song),
    })
}

fn channels(gate: Option<Arc<Semaphore>>) -> Vec<Arc<dyn MusicChannel>> {
    let ch: Arc<dyn MusicChannel> = Arc::new(FakeChannel::new(gate));
    vec![ch]
}

#[tokio::test]
async fn submit_and_done_ok() -> color_eyre::Result<()> {
    let sched = Scheduler::new(&channels(None));
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
    let sched = Scheduler::new(&channels(Some(gate)));
    let h = sched.submit(my_playlists_kind(), Priority::User);
    h.cancel();
    assert_eq!(h.done().await, TaskOutcome::Cancelled);
    Ok(())
}

#[tokio::test]
async fn dedup_returns_same_handle() -> color_eyre::Result<()> {
    let gate = Arc::new(Semaphore::new(0));
    let sched = Scheduler::new(&channels(Some(Arc::clone(&gate))));
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
    let sched = Scheduler::new(&channels(Some(gate)));
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
    let sched = Scheduler::new(&channels(Some(Arc::clone(&gate))));
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
    let sched = Scheduler::new(&channels(None));
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
    let sched = Scheduler::new(&channels(Some(Arc::clone(&gate))));
    let h1 = sched.submit(song_url_kind("s2"), Priority::User);
    let h2 = sched.submit(song_url_kind("s2"), Priority::User);
    assert_eq!(h1.id, h2.id);

    gate.add_permits(1);
    assert_eq!(h1.done().await, TaskOutcome::Ok);
    Ok(())
}

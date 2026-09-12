//! Song submission, deduplication, and playlist snapshot expansion.

use mineral_model::Song;
use mineral_protocol::{
    DownloadId, DownloadOrigin, DownloadStatus, DownloadTarget, PlaylistRef, SongDownloadView,
    ToastKind,
};

use super::DownloadManager;
use super::state::{DownloadKey, SongDownload};

/// Scheduler lane of a newly admitted row.
#[derive(Clone, Copy)]
pub(super) enum Lane {
    /// Direct Song submission.
    Direct,

    /// Song expanded from a playlist.
    Playlist,
}

impl DownloadManager {
    /// Accepts a Song immediately or starts asynchronous playlist expansion.
    pub(crate) fn submit(&self, target: DownloadTarget) {
        match target {
            DownloadTarget::Song(song) => {
                self.admit_song(&song, DownloadOrigin::Direct, Lane::Direct);
            }
            DownloadTarget::Playlist(id) => {
                {
                    let mut state = self.inner.state.lock();
                    state.preparing_playlists = state.preparing_playlists.saturating_add(1);
                }
                self.bump();
                let manager = self.clone();
                tokio::spawn(async move { manager.expand_playlist(id).await });
            }
        }
    }

    /// Expands one canonical playlist snapshot into flat Song admissions.
    async fn expand_playlist(&self, id: mineral_model::PlaylistId) {
        let channel = self
            .inner
            .runtime
            .channels
            .iter()
            .find(|channel| channel.source() == id.namespace())
            .cloned();
        let result = match channel {
            Some(channel) => channel.playlist_detail(&id).await.map_err(|error| {
                mineral_log::warn!(target: "download", playlist_id = %id.qualified(), error = mineral_log::chain(&error), "playlist expansion failed");
                "Download failed: could not load playlist".to_owned()
            }),
            None => Err("Download failed: source has no catalog channel".to_owned()),
        };
        match result {
            Ok(playlist) if !self.inner.shutdown.is_cancelled() => {
                let origin = DownloadOrigin::Playlist(PlaylistRef {
                    id: playlist.id,
                    name: playlist.name,
                });
                for entry in playlist.entries {
                    self.admit_song(&entry.song, origin.clone(), Lane::Playlist);
                }
            }
            Ok(_) => {}
            Err(message) if !self.inner.shutdown.is_cancelled() => {
                self.inner.runtime.notify.toast(ToastKind::Warn, message);
            }
            Err(_) => {}
        }
        {
            let mut state = self.inner.state.lock();
            state.preparing_playlists = state.preparing_playlists.saturating_sub(1);
            state.finish_wave_if_idle();
        }
        self.inner.wake.notify_one();
        self.bump();
    }

    /// Inserts or deduplicates one flat Song row.
    pub(super) fn admit_song(&self, song: &Song, origin: DownloadOrigin, lane: Lane) {
        let mut state = self.inner.state.lock();
        let key = DownloadKey {
            export_root: self.inner.runtime.music_dir.clone(),
            song_id: song.id.clone(),
            quality: state.quality,
        };
        if let Some(existing) = state.dedup.get(&key) {
            mineral_log::info!(
                target: "download",
                download_id = %existing,
                song_id = %song.id.qualified(),
                source = song.source().name(),
                "download submission deduplicated"
            );
            return;
        }
        let id = DownloadId::new(uuid::Uuid::new_v4().to_string());
        let admission_order = state.next_admission;
        state.next_admission = state.next_admission.wrapping_add(1);
        let quality = state.quality;
        let row = SongDownload {
            view: SongDownloadView {
                id: id.clone(),
                song: Box::new(song.clone()),
                origin,
                status: DownloadStatus::Queued,
                quality,
                bytes_done: 0,
                bytes_total: None,
                speed_bps: 0,
                failure: None,
            },
            key: key.clone(),
            admission_order,
        };
        state.rows.insert(id.clone(), row);
        state.dedup.insert(key, id.clone());
        match lane {
            Lane::Direct => state.direct_queue.push_back(id.clone()),
            Lane::Playlist => state.playlist_queue.push_back(id.clone()),
        }
        state.open_wave();
        drop(state);
        mineral_log::info!(
            target: "download",
            download_id = %id,
            song_id = %song.id.qualified(),
            source = song.source().name(),
            "Song download admitted"
        );
        self.inner.wake.notify_one();
        self.bump();
    }
}

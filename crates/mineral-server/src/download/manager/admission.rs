//! Song admission and deduplication after commands or playlist expansion results arrive.

use mineral_channel_core::{PlaylistDetail, finish_playlist};
use mineral_model::{CachePolicy, PlaylistId, Song};
use mineral_protocol::{
    DownloadId, DownloadOrigin, DownloadStatus, FailureNotice, PlaylistRef, SongDownloadView,
};
use uuid::Uuid;

use super::lifecycle::DownloadRuntime;
use super::state::{DownloadKey, ManagerState, SongDownload};

/// Failure while expanding a playlist for downloads.
#[derive(Debug, thiserror::Error)]
pub(super) enum ExpansionError {
    /// No catalog channel serves the playlist source.
    #[error("no catalog channel for playlist source")]
    NoChannel,

    /// Catalog lookup failed.
    #[error("playlist lookup failed")]
    Channel(#[from] mineral_channel_core::Error),
}

/// Scheduler lane of a newly admitted row.
#[derive(Clone, Copy)]
pub(super) enum Lane {
    /// Direct Song submission.
    Direct,

    /// Song expanded from a playlist.
    Playlist,
}

/// Fetches a complete playlist without accessing manager state.
pub(super) async fn expand_playlist(
    runtime: &DownloadRuntime,
    id: PlaylistId,
) -> (PlaylistId, Result<PlaylistDetail, ExpansionError>) {
    let channel = runtime
        .channels
        .iter()
        .find(|channel| channel.source() == id.namespace());
    let result = match channel {
        Some(channel) => finish_playlist(channel.playlist_detail(&id, CachePolicy::UseCache))
            .await
            .map_err(ExpansionError::from),
        None => Err(ExpansionError::NoChannel),
    };
    (id, result)
}

/// Admits the fetched songs using the config in effect when expansion completes.
pub(super) fn apply_playlist(
    state: &mut ManagerState,
    runtime: &DownloadRuntime,
    id: PlaylistId,
    result: Result<PlaylistDetail, ExpansionError>,
) {
    state.preparing_playlists -= 1;
    match result {
        Ok(playlist) => {
            let origin = DownloadOrigin::Playlist(PlaylistRef {
                id: playlist.playlist.id,
                name: playlist.playlist.name,
            });
            for entry in playlist.playlist.entries {
                admit_song(state, runtime, &entry.song, origin.clone(), Lane::Playlist);
            }
        }
        Err(error) => {
            mineral_log::warn!(target: "download", playlist_id = %id.qualified(), error = mineral_log::chain(&error), "playlist expansion failed");
            runtime
                .notify
                .failure(FailureNotice::PlaylistDownloadFailed { id });
        }
    }
}

/// Deduplicates queued/active identities without promoting an existing playlist row.
pub(super) fn admit_song(
    state: &mut ManagerState,
    runtime: &DownloadRuntime,
    song: &Song,
    origin: DownloadOrigin,
    lane: Lane,
) {
    let key = DownloadKey {
        export_root: runtime.music_dir.clone(),
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
    let id = DownloadId::new(Uuid::new_v4().to_string());
    let admission_order = state.next_admission;
    state.next_admission = state.next_admission.next();
    let row = SongDownload {
        view: SongDownloadView {
            id: id.clone(),
            song: Box::new(song.clone()),
            origin,
            status: DownloadStatus::Queued,
            quality: state.quality,
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
    mineral_log::info!(
        target: "download",
        download_id = %id,
        song_id = %song.id.qualified(),
        source = song.source().name(),
        "Song download admitted"
    );
}

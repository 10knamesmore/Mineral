//! In-memory state owned by the Song download manager.

use std::collections::VecDeque;
use std::path::PathBuf;

use mineral_model::{BitRate, SongId};
use mineral_protocol::{
    DownloadId, DownloadStatus, DownloadSummary, DownloadWave, SongDownloadView,
};
use rustc_hash::FxHashMap;
use tokio_util::sync::CancellationToken;

/// Maximum terminal rows retained for the current daemon session.
const TERMINAL_LIMIT: usize = 100;

/// Mutable manager-owned state.
pub(super) struct ManagerState {
    /// Rows keyed by session identity.
    pub(super) rows: FxHashMap<DownloadId, SongDownload>,

    /// User-submitted Song lane, FIFO and preferred at each free slot.
    pub(super) direct_queue: VecDeque<DownloadId>,

    /// Playlist-expanded Song lane, FIFO.
    pub(super) playlist_queue: VecDeque<DownloadId>,

    /// Cancellation handles retained until their writers have exited.
    pub(super) active: FxHashMap<DownloadId, CancellationToken>,

    /// Queued/active duplicate keys.
    pub(super) dedup: FxHashMap<DownloadKey, DownloadId>,

    /// Terminal row identities from oldest to newest.
    pub(super) terminal: VecDeque<DownloadId>,

    /// Monotonic admission order.
    pub(super) next_admission: u64,

    /// Download quality applied to new admissions.
    pub(super) quality: BitRate,

    /// Maximum active Song attempts.
    pub(super) max_concurrent: usize,

    /// Playlist snapshots currently being expanded.
    pub(super) preparing_playlists: usize,

    /// Whether a wave currently has unsettled Song downloads.
    pub(super) wave_open: bool,

    /// Counts for the current unsettled wave.
    pub(super) wave: DownloadWave,

    /// Latest settled wave retained for client deduplication.
    pub(super) latest_wave: Option<DownloadWave>,
}

/// Internal row with scheduler metadata not exposed over IPC.
pub(super) struct SongDownload {
    /// Client-facing state.
    pub(super) view: SongDownloadView,

    /// Active dedup identity.
    pub(super) key: DownloadKey,

    /// Monotonic admission order.
    pub(super) admission_order: u64,
}

/// Duplicate identity for one export request.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DownloadKey {
    /// Export root participates because it is part of file identity.
    pub(super) export_root: Option<PathBuf>,

    /// Globally namespaced Song identity.
    pub(super) song_id: SongId,

    /// Requested quality before hook rewriting.
    pub(super) quality: BitRate,
}

impl ManagerState {
    /// Creates empty session state.
    pub(super) fn new(quality: BitRate, max_concurrent: usize) -> Self {
        Self {
            rows: FxHashMap::default(),
            direct_queue: VecDeque::new(),
            playlist_queue: VecDeque::new(),
            active: FxHashMap::default(),
            dedup: FxHashMap::default(),
            terminal: VecDeque::new(),
            next_admission: 0,
            quality,
            max_concurrent,
            preparing_playlists: 0,
            wave_open: false,
            wave: DownloadWave::default(),
            latest_wave: None,
        }
    }

    /// Returns the small per-tick summary.
    pub(super) fn summary(&self) -> DownloadSummary {
        let queued = self
            .rows
            .values()
            .filter(|row| row.view.status == DownloadStatus::Queued)
            .count();
        let speed_bps = self
            .rows
            .values()
            .filter(|row| row.view.status == DownloadStatus::Downloading)
            .fold(0u64, |sum, row| sum.saturating_add(row.view.speed_bps));
        DownloadSummary {
            active: self.active.len(),
            queued,
            preparing_playlists: self.preparing_playlists,
            speed_bps,
            latest_wave: self.latest_wave.clone(),
        }
    }

    /// Returns active, queued, then newest-first terminal rows.
    pub(super) fn snapshot(&self) -> Vec<SongDownloadView> {
        let mut rows = self
            .rows
            .values()
            .filter(|row| !row.view.status.terminal())
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| {
            (
                row.view.status == DownloadStatus::Queued,
                row.admission_order,
            )
        });
        rows.into_iter()
            .chain(
                self.terminal
                    .iter()
                    .rev()
                    .filter_map(|id| self.rows.get(id)),
            )
            .map(|row| row.view.clone())
            .collect()
    }

    /// Starts a result wave when the manager was previously settled.
    pub(super) fn open_wave(&mut self) {
        if !self.wave_open {
            self.wave_open = true;
            self.wave = DownloadWave::default();
        }
    }

    /// Marks a row terminal, removes its dedup identity, updates wave counts, and prunes history.
    pub(super) fn mark_terminal(
        &mut self,
        id: &DownloadId,
        status: DownloadStatus,
        failure: Option<String>,
    ) {
        let Some(row) = self.rows.get_mut(id) else {
            return;
        };
        if row.view.status.terminal() {
            return;
        }
        row.view.status = status;
        row.view.speed_bps = 0;
        row.view.failure = failure;
        let key = row.key.clone();
        self.dedup.remove(&key);
        self.terminal.push_back(id.clone());
        match status {
            DownloadStatus::Downloaded => {
                self.wave.downloaded = self.wave.downloaded.saturating_add(1);
            }
            DownloadStatus::AlreadyPresent => {
                self.wave.already_present = self.wave.already_present.saturating_add(1);
            }
            DownloadStatus::SkippedByHook => {
                self.wave.skipped_by_hook = self.wave.skipped_by_hook.saturating_add(1);
            }
            DownloadStatus::Failed => {
                self.wave.failed = self.wave.failed.saturating_add(1);
            }
            DownloadStatus::Stopped => {
                self.wave.stopped = self.wave.stopped.saturating_add(1);
            }
            DownloadStatus::Queued
            | DownloadStatus::Resolving
            | DownloadStatus::Downloading
            | DownloadStatus::Finalizing
            | DownloadStatus::Stopping => {}
        }
        while self.terminal.len() > TERMINAL_LIMIT {
            if let Some(oldest) = self.terminal.pop_front() {
                self.rows.remove(&oldest);
            }
        }
    }

    /// Publishes a wave once no Song or playlist expansion remains unsettled.
    pub(super) fn finish_wave_if_idle(&mut self) {
        let queued = self
            .rows
            .values()
            .any(|row| row.view.status == DownloadStatus::Queued);
        if !self.wave_open || queued || !self.active.is_empty() || self.preparing_playlists > 0 {
            return;
        }
        let previous = self.latest_wave.as_ref().map_or(0, |wave| wave.sequence);
        self.wave.sequence = previous.wrapping_add(1);
        self.latest_wave = Some(self.wave.clone());
        self.wave_open = false;
    }
}

/// Pops the first still-queued identity while discarding stale queue entries.
pub(super) fn pop_queued(
    queue: &mut VecDeque<DownloadId>,
    rows: &FxHashMap<DownloadId, SongDownload>,
) -> Option<DownloadId> {
    while let Some(id) = queue.pop_front() {
        if rows
            .get(&id)
            .is_some_and(|row| row.view.status == DownloadStatus::Queued)
        {
            return Some(id);
        }
    }
    None
}

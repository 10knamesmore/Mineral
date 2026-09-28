//! Computes envelopes for opened local media and replays normal-song persistent caches.
//!
//! Hook replacements retain envelopes only on their playback slot. Normal-song envelopes
//! remain persistent; every async delivery still targets the instance that requested it.

use std::path::PathBuf;

use mineral_model::{Envelope, SongId};

use crate::gapless::PrefetchState;
use crate::playback_instance::PlaybackSlot;
use crate::player::PlayerCore;

impl PlayerCore {
    /// Caches an envelope for a complete, non-substituted capture and its live normal slots.
    /// Decode and storage failures only log; clients can render without an envelope.
    ///
    /// # Params:
    ///   - `song_id`: Identity of the captured original media.
    ///   - `path`: Fully readable cache or download file, never a partial capture.
    pub(crate) fn ensure_envelope(&self, song_id: SongId, path: PathBuf) {
        let slots = self.with_state(|state| {
            let mut slots = Vec::<PlaybackSlot>::new();
            if let Some(slot) = &state.current_slot
                && slot.song_id == song_id
                && state
                    .media_info
                    .as_ref()
                    .is_some_and(|info| !info.substituted)
            {
                slots.push(slot.clone());
            }
            if let PrefetchState::Armed { slot, queued } = &state.prefetch
                && slot.song_id == song_id
                && !queued.media_info.substituted
            {
                slots.push(slot.clone());
            }
            slots
        });
        if slots.is_empty() {
            self.ensure_cached_envelope(song_id, path, None);
        } else {
            for slot in slots {
                self.ensure_cached_envelope(song_id.clone(), path.clone(), Some(slot));
            }
        }
    }

    /// Computes a replacement envelope in memory, or loads/caches original media normally.
    /// Starts only after opened media has been committed to the current or prefetch slot.
    /// Cancellation discards the result; an already-running blocking decoder may finish.
    ///
    /// # Params:
    ///   - `slot`: Instance owning the opened media, retained through gapless promotion.
    ///   - `path`: Fully readable local media file.
    ///   - `substituted`: Final opened-media fact selecting instance-only storage.
    pub(crate) fn ensure_playback_envelope(
        &self,
        slot: &PlaybackSlot,
        path: PathBuf,
        substituted: bool,
    ) {
        if !substituted {
            self.ensure_cached_envelope(slot.song_id.clone(), path, Some(slot.clone()));
            return;
        }
        let player = self.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            mineral_log::debug!(
                target: "player",
                song_id = %slot.song_id.qualified(),
                instance_id = %slot.instance_id,
                path = %path.display(),
                "compute instance-only replacement envelope"
            );
            let computed = tokio::select! {
                biased;
                () = slot.cancellation.cancelled() => {
                    mineral_log::debug!(
                        target: "player",
                        song_id = %slot.song_id.qualified(),
                        instance_id = %slot.instance_id,
                        "replacement envelope cancelled"
                    );
                    return;
                }
                computed = player.decode_envelope(&slot.song_id, path) => computed,
            };
            if let Some(envelope) = computed {
                player.deliver_envelope(&slot, envelope);
            }
        });
    }

    /// Replays a normal-song cache only for already-opened current media without an envelope.
    /// Reconnecting clients receive replacement envelopes directly from the current slot.
    pub(crate) fn replay_current_envelope(&self) {
        let Some(slot) = self.with_state(|state| {
            let slot = state.current_slot.as_ref()?;
            if state.media_info.as_ref()?.substituted
                || slot.envelope.is_some()
                || slot.cancellation.is_cancelled()
            {
                return None;
            }
            Some(slot.clone())
        }) else {
            return;
        };
        let player = self.clone();
        tokio::spawn(async move {
            if let Some(envelope) = player.cached_envelope(&slot.song_id).await {
                player.deliver_envelope(&slot, envelope);
            }
        });
    }

    /// Reads or computes a persistent envelope, freezing its optional delivery target at entry.
    fn ensure_cached_envelope(&self, song_id: SongId, path: PathBuf, slot: Option<PlaybackSlot>) {
        let player = self.clone();
        tokio::spawn(async move {
            match player.cached_envelope(&song_id).await {
                Some(envelope) => {
                    if let Some(slot) = slot {
                        player.deliver_envelope(&slot, envelope);
                    }
                }
                None => player.compute_cached_envelope(song_id, path, slot).await,
            }
        });
    }

    /// Reads the current algorithm version; storage failures log and count as cache misses.
    async fn cached_envelope(&self, song_id: &SongId) -> Option<Envelope> {
        let scope = self.persist().scope(song_id.namespace());
        match scope
            .get_envelope(song_id, mineral_audio::ENVELOPE_VERSION)
            .await
        {
            Ok(hit) => hit,
            Err(error) => {
                mineral_log::warn!(
                    target: "player",
                    song_id = %song_id.qualified(),
                    error = mineral_log::chain(&error),
                    "read envelope cache failed"
                );
                None
            }
        }
    }

    /// Computes and persists original media, deduplicating only requests sharing a delivery owner.
    async fn compute_cached_envelope(
        &self,
        song_id: SongId,
        path: PathBuf,
        slot: Option<PlaybackSlot>,
    ) {
        let inflight_key = slot.as_ref().map_or_else(
            || format!("song:{}", song_id.qualified()),
            |slot| format!("playback:{}", slot.instance_id),
        );
        if !self
            .inner
            .envelope_inflight
            .lock()
            .insert(inflight_key.clone())
        {
            return;
        }
        mineral_log::debug!(
            target: "player",
            song_id = %song_id.qualified(),
            path = %path.display(),
            "compute persistent song envelope"
        );
        let computed = self.decode_envelope(&song_id, path).await;
        if let Some(envelope) = &computed {
            let scope = self.persist().scope(song_id.namespace());
            if let Err(error) = scope.put_envelope(&song_id, envelope).await {
                // A failed write does not prevent this instance from displaying the envelope.
                mineral_log::warn!(
                    target: "player",
                    song_id = %song_id.qualified(),
                    error = mineral_log::chain(&error),
                    "write envelope cache failed"
                );
            }
        }
        self.inner.envelope_inflight.lock().remove(&inflight_key);
        if let (Some(slot), Some(envelope)) = (slot, computed) {
            self.deliver_envelope(&slot, envelope);
        }
    }

    /// Decodes a complete local file without accessing persistent storage.
    async fn decode_envelope(&self, song_id: &SongId, path: PathBuf) -> Option<Envelope> {
        let params = self.inner.envelope_params.clone();
        match tokio::task::spawn_blocking(move || mineral_audio::envelope_from_file(&path, &params))
            .await
        {
            Ok(Ok(envelope)) => Some(envelope),
            Ok(Err(error)) => {
                mineral_log::warn!(
                    target: "player",
                    song_id = %song_id.qualified(),
                    error = mineral_log::chain(&error),
                    "compute envelope failed"
                );
                None
            }
            Err(error) => {
                mineral_log::warn!(
                    target: "player",
                    song_id = %song_id.qualified(),
                    error = mineral_log::chain(&error),
                    "envelope worker join failed"
                );
                None
            }
        }
    }

    /// Publishes only to the matching live instance, whether current or prefetched.
    fn deliver_envelope(&self, slot: &PlaybackSlot, envelope: Envelope) {
        let adopted = self
            .with_state(|state| state.adopt_envelope(slot.instance_id, &slot.song_id, envelope));
        mineral_log::debug!(
            target: "player",
            song_id = %slot.song_id.qualified(),
            instance_id = %slot.instance_id,
            adopted,
            "playback envelope delivery"
        );
    }
}

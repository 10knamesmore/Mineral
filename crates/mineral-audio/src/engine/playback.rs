//! Audio control and snapshots. Decoder workers and CPAL share one playback state.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use mineral_playback::OpenedMedia;
use parking_lot::Mutex;

use super::output::Output;
use super::render::{Decoding, SharedPlayback, Track, TrackSlot};
use super::worker;
use crate::command::{AudioCommand, SeekRequest};
use crate::snapshot::{AudioBackend, AudioSnapshot};
use crate::{Bps, Error, Result};

pub(crate) fn pct_to_gain(percent: u8) -> f32 {
    let ratio = f32::from(percent.min(100)) / 100.0;
    ratio * ratio * ratio
}

pub(super) struct Engine {
    output: Output,

    playback: Arc<SharedPlayback>,

    next_id: u64,

    tick: Duration,
}

impl Engine {
    pub(super) fn new(output: Output, playback: Arc<SharedPlayback>, tick: Duration) -> Self {
        Self {
            output,
            playback,
            next_id: 0,
            tick,
        }
    }

    pub(super) fn handle_command(&mut self, command: AudioCommand) {
        self.output.refresh_device();
        match command {
            AudioCommand::Play(media) => {
                self.stop();
                if let Err(error) = self.install(media, false) {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "play error");
                }
            }
            AudioCommand::AppendNext(media) => {
                if let Err(error) = self.install(media, true) {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "prefetch error");
                }
            }
            AudioCommand::ClearNext => {
                let mut state = self.playback.state.lock();
                if let Some(index) = state.next.take() {
                    *state.track_mut(index) = None;
                }
            }
            AudioCommand::Pause => self.playback.state.lock().paused = true,
            AudioCommand::Resume => self.playback.state.lock().paused = false,
            AudioCommand::Stop => self.stop(),
            AudioCommand::SetVolume(percent) => {
                self.playback.state.lock().gain = pct_to_gain(percent);
            }
            AudioCommand::ListOutputs(reply) => {
                let _ = reply.send(Output::devices());
            }
            AudioCommand::SelectOutput { target, reply } => {
                let _ = reply.send(self.output.select(target));
            }
        }
    }

    /// Allocate and publish a track before its worker starts. Only the engine destroys slots.
    fn install(&mut self, media: OpenedMedia, prefetched: bool) -> Result<()> {
        if self.output.info().is_none() {
            media.cancellation().cancel();
            return Err(Error::OutputUnavailable);
        }
        self.next_id += 1;
        let id = self.next_id;
        let mut state = self.playback.state.lock();
        let index = if prefetched {
            let Some(current) = state.current else {
                media.cancellation().cancel();
                return Ok(());
            };
            current.other()
        } else {
            TrackSlot::First
        };
        let Some(output) = &state.output else {
            media.cancellation().cancel();
            return Err(Error::OutputUnavailable);
        };
        let capacity = output.capacity_samples;
        *state.track_mut(index) = Some(Track {
            id,
            generation: 0,
            cancellation: media.cancellation().clone(),
            transfer: media.transfer().cloned(),
            source_format: None,
            duration: None,
            samples: VecDeque::with_capacity(capacity),
            origin: Duration::ZERO,
            rendered_frames: 0,
            seek_position: None,
            decoding: Decoding::Reading,
            finished: false,
        });
        if prefetched {
            state.next = Some(index);
        } else {
            state.current = Some(index);
            state.track_token += 1;
            state.paused = false;
        }
        drop(state);
        if let Err(error) = worker::spawn(media, Arc::clone(&self.playback), id, self.tick) {
            let mut state = self.playback.state.lock();
            *state.track_mut(index) = None;
            if prefetched {
                state.next = None;
            } else {
                state.current = None;
            }
            return Err(error);
        }
        Ok(())
    }

    fn stop(&self) {
        self.playback.state.lock().stop();
    }

    pub(super) fn drain_seek(&self, mailbox: &Arc<Mutex<Option<SeekRequest>>>) {
        let Some(request) = mailbox.lock().take() else {
            return;
        };
        let mut state = self.playback.state.lock();
        mineral_log::debug!(
            target: "audio",
            requested_track = request.track_token,
            current_track = state.track_token,
            position = ?request.position,
            "seek requested"
        );
        if request.track_token != state.track_token {
            return;
        }
        let rate = state.output.as_ref().map(|output| output.pcm.sample_rate);
        if let Some(rate) = rate
            && let Some(index) = state.current
            && let Some(track) = state.track_mut(index).as_mut()
            && !matches!(track.decoding, Decoding::Failed(_))
        {
            track.rewind(request.position, rate);
            state.rendering = false;
        }
        drop(state);
    }

    pub(super) fn update_snapshot(&mut self, snapshot: &Arc<Mutex<AudioSnapshot>>) {
        self.output.refresh_device();
        let mut state = self.playback.state.lock();
        if let Some(index) = state.current
            && let Some(track) = state.track(index)
            && let Decoding::Failed(error) = &track.decoding
        {
            mineral_log::error!(target: "audio", error = mineral_log::chain(error.as_ref()), "playback stopped");
            state.stop();
        }
        if let Some(index) = state.next
            && state
                .track(index)
                .as_ref()
                .is_some_and(|track| matches!(track.decoding, Decoding::Failed(_)))
        {
            state.next = None;
            *state.track_mut(index) = None;
        }
        for slot in &mut state.tracks {
            if slot.as_ref().is_some_and(|track| track.finished) {
                *slot = None;
            }
        }
        let current = state.current.and_then(|index| state.track(index).as_ref());
        let next = state.next.and_then(|index| state.track(index).as_ref());
        let position = current
            .zip(state.output.as_ref())
            .map_or(state.last_position, |(track, output)| {
                track.position(output.pcm.sample_rate)
            });
        let position_ms = duration_to_ms(position);
        let playing = !state.paused && current.is_some() && self.output.info().is_some();
        {
            let mut snapshot = snapshot.lock();
            snapshot.output = self.output.info().cloned();
            snapshot.backend = if snapshot.output.is_some() {
                AudioBackend::Device
            } else {
                AudioBackend::Null
            };
            snapshot.playing = playing;
            snapshot.buffering = playing && !state.rendering;
            snapshot.position_ms = position_ms;
            snapshot.duration_ms = current.and_then(|track| track.duration).map(duration_to_ms);
            snapshot.track_finished_seq = state.finished_seq;
            snapshot.current_track_token = state.track_token;
            snapshot.buffered_bps = buffered(current);
            snapshot.next_duration_ms = next.and_then(|track| track.duration).map(duration_to_ms);
            snapshot.next_buffered_bps = buffered(next);
            snapshot.next_ready = next.is_some_and(|track| !track.samples.is_empty());
            snapshot.sample_rate_hz = current
                .and_then(|track| track.source_format)
                .map_or(0, |format| format.sample_rate.get());
        }
        drop(state);
        self.output.recover_if_stalled(playing, position_ms);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn buffered(track: Option<&Track>) -> Bps {
    match track {
        None => Bps::ZERO,
        Some(track) => match &track.transfer {
            None => Bps::FULL,
            Some(transfer) => {
                let snapshot = transfer.snapshot();
                if snapshot.complete {
                    Bps::FULL
                } else {
                    snapshot
                        .total
                        .map_or(Bps::ZERO, |total| Bps::ratio(snapshot.downloaded, total))
                }
            }
        },
    }
}

fn duration_to_ms(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_mul(1_000)
        .saturating_add(u64::from(duration.subsec_millis()))
}

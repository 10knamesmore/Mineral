//! Playback commands, decoder queue accounting, and shared audio snapshots.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use mineral_playback::{OpenedMedia, SeekSupport};
use parking_lot::Mutex;
use rodio::Source;

use super::decoding::{InstanceSource, build_decoder};
use super::output::Output;
use crate::command::AudioCommand;
use crate::queue_slots::{Boundary, PlayHead, Slot};
use crate::snapshot::{AudioBackend, AudioSnapshot};
use crate::tap::{SharedProd, TapSource};

/// Maps a UI volume percentage onto perceptual cubic gain.
pub(super) fn pct_to_gain(percent: u8) -> f32 {
    let ratio = f32::from(percent.min(100)) / 100.0;
    ratio * ratio * ratio
}

/// Mutable state owned by the audio engine thread.
pub(super) struct Engine {
    /// System output stream and rodio queue.
    output: Output,

    /// Shared PCM tap producer.
    tap_producer: SharedProd,

    /// Current sample rate shared with the spectrum consumer.
    sample_rate: Arc<AtomicU32>,

    /// 累计尝试写入 tap 的 mono 样本数(缺口辨认)。
    tap_pushed: Arc<AtomicU64>,

    /// Current and gapless-prefetched decoder accounting.
    head: PlayHead,
}

impl Engine {
    /// Creates engine state around an initialized output.
    pub(super) fn new(
        output: Output,
        tap_producer: &SharedProd,
        sample_rate: &Arc<AtomicU32>,
        tap_pushed: &Arc<AtomicU64>,
    ) -> Self {
        Self {
            output,
            tap_producer: Arc::clone(tap_producer),
            sample_rate: Arc::clone(sample_rate),
            tap_pushed: Arc::clone(tap_pushed),
            head: PlayHead::default(),
        }
    }

    /// Applies one command and contains command-local failures.
    pub(super) fn handle_command(&mut self, command: AudioCommand) {
        self.output.refresh_device();
        match command {
            AudioCommand::Play(media) => {
                if let Err(error) = self.play(media) {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "play error");
                }
            }
            AudioCommand::AppendNext(media) => {
                if let Err(error) = self.append_next(media) {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "prefetch decode error");
                }
            }
            AudioCommand::ListOutputs(reply) => {
                let _ = reply.send(Output::devices());
            }
            AudioCommand::SelectOutput { target, reply } => {
                let result = self.output.select(target);
                if let Err(error) = &result {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(error), "audio output selection failed");
                }
                let _ = reply.send(result);
            }
            AudioCommand::ClearNext => self.clear_next(),
            AudioCommand::Pause => self.output.player().pause(),
            AudioCommand::Resume => self.output.player().play(),
            AudioCommand::Stop => self.stop(),
            AudioCommand::SetVolume(percent) => {
                self.output.player().set_volume(pct_to_gain(percent));
            }
        }
    }

    /// Replaces the output queue with one already-opened current media item.
    fn play(&mut self, media: OpenedMedia) -> color_eyre::Result<()> {
        // rodio append after stop waits for the callback to drain the previous queue.
        if self.output.info().is_none() {
            media.cancellation().cancel();
            return Err(color_eyre::eyre::eyre!("audio output is unavailable"));
        }
        let song_id = media.info().song_id.qualified();
        self.output.player().stop();
        self.head.stop();
        self.sample_rate.store(0, Ordering::Relaxed);
        let slot = self.append_opened(media, "current")?;
        mineral_log::info!(target: "audio", song_id, "start decoding");
        self.output.player().play();
        self.sample_rate.store(slot.sample_rate, Ordering::Relaxed);
        self.head.start(slot);
        Ok(())
    }

    /// Appends already-opened next media behind current without reopening it.
    fn append_next(&mut self, media: OpenedMedia) -> color_eyre::Result<()> {
        if !self.head.cur.occupied {
            media.cancellation().cancel();
            return Ok(());
        }
        self.head.clear_next();
        let slot = self.append_opened(media, "next")?;
        self.head.arm_next(slot);
        Ok(())
    }

    /// Builds and appends one decoder, returning its slot accounting.
    fn append_opened(
        &mut self,
        media: OpenedMedia,
        slot_name: &'static str,
    ) -> color_eyre::Result<Slot> {
        let byte_len = (media.seek_support() == SeekSupport::RandomAccess)
            .then_some(media.byte_len())
            .flatten();
        let transfer = media.transfer().cloned();
        let cancellation = media.cancellation().clone();
        let reader = media.into_reader();
        let decoder = build_decoder(reader, byte_len)?;
        let duration_ms = decoder.total_duration().map(duration_to_ms);
        let sample_rate = u32::from(decoder.sample_rate());
        mineral_log::info!(
            target: "audio",
            slot = slot_name,
            sample_rate,
            duration_ms = ?duration_ms,
            byte_len_known = byte_len.is_some(),
            "decoder ready"
        );
        let source = InstanceSource::new(decoder, cancellation.clone());
        self.output.player().append(TapSource::new(
            source,
            Arc::clone(&self.tap_producer),
            Arc::clone(&self.tap_pushed),
        ));
        Ok(Slot {
            duration_ms,
            sample_rate,
            transfer,
            cancellation: Some(cancellation),
            occupied: true,
        })
    }

    /// Cancels an armed prefetched decoder; its source drains silently when reached.
    fn clear_next(&mut self) {
        self.head.clear_next();
    }

    /// Stops output and cancels both decoder slots.
    fn stop(&mut self) {
        self.head.stop();
        self.output.player().stop();
        self.sample_rate.store(0, Ordering::Relaxed);
    }

    /// Applies one pending latest-wins seek request.
    pub(super) fn drain_seek(&self, mailbox: &Arc<Mutex<Option<Duration>>>) {
        let Some(target) = mailbox.lock().take() else {
            return;
        };
        // A disconnected stream cannot acknowledge rodio's synchronous seek mailbox.
        if self.output.info().is_none() {
            mineral_log::warn!(target: "audio", seek_to = ?target, "seek skipped because audio output is unavailable");
            return;
        }
        if let Err(error) = self.output.player().try_seek(target) {
            mineral_log::warn!(
                target: "audio",
                seek_to = ?target,
                error = mineral_log::chain(&error),
                "seek failed"
            );
        }
    }

    /// Updates shared playback state and observes natural decoder boundaries.
    pub(super) fn update_snapshot(&mut self, snapshot: &Arc<Mutex<AudioSnapshot>>) {
        self.output.refresh_device();
        let position_ms = duration_to_ms(self.output.player().get_pos());
        let paused = self.output.player().is_paused();
        let boundary = self.head.observe(self.output.player().len());
        if boundary == Boundary::Gapless {
            self.sample_rate
                .store(self.head.cur.sample_rate, Ordering::Relaxed);
        }
        let playing = !paused && self.head.cur.occupied && self.output.info().is_some();
        self.output.recover_if_stalled(playing, position_ms);
        let fields = self.head.snapshot_fields();
        let mut current = snapshot.lock();
        current.output = self.output.info().cloned();
        current.backend = if current.output.is_some() {
            AudioBackend::Device
        } else {
            AudioBackend::Null
        };
        current.playing = playing;
        current.position_ms = position_ms;
        current.duration_ms = fields.duration_ms;
        current.track_finished_seq = fields.track_finished_seq;
        current.current_track_token = fields.current_track_token;
        current.buffered_bps = fields.buffered_bps;
        current.next_duration_ms = fields.next_duration_ms;
        current.next_buffered_bps = fields.next_buffered_bps;
        current.next_ready = fields.next_ready;
        current.sample_rate_hz = self.head.cur.sample_rate;
    }
}

/// Converts a duration to milliseconds with saturation on impossible overflow.
fn duration_to_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

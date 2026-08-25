//! Audio engine thread consuming already-opened encoded media.

mod output;

use std::io::{Read, Seek};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use color_eyre::eyre::eyre;
use mineral_playback::{MediaReader, OpenedMedia, SeekSupport};
use parking_lot::Mutex;
use rodio::Source;
use rodio::decoder::DecoderBuilder;
use rodio::source::SeekError;
use tokio_util::sync::CancellationToken;

use crate::capture::{CaptureReader, CaptureState};
use crate::command::AudioCommand;
use crate::engine::output::Output;
use crate::handle::{AudioMode, EngineParams};
use crate::queue_slots::{Boundary, PlayHead, Slot};
use crate::snapshot::{AudioBackend, AudioSnapshot};
use crate::tap::{SharedProd, TapSource};

/// Maps a UI volume percentage onto perceptual cubic gain.
fn pct_to_gain(percent: u8) -> f32 {
    let ratio = f32::from(percent.min(100)) / 100.0;
    ratio * ratio * ratio
}

/// Shared handles transferred into the dedicated audio thread.
pub(crate) struct EngineIo {
    /// Latest audio snapshot.
    pub(crate) snapshot: Arc<Mutex<AudioSnapshot>>,

    /// Latest-wins seek mailbox.
    pub(crate) seek_mailbox: Arc<Mutex<Option<Duration>>>,

    /// Engine startup result channel.
    pub(crate) ready_tx: mpsc::SyncSender<color_eyre::Result<()>>,

    /// PCM spectrum tap producer.
    pub(crate) tap_producer: SharedProd,

    /// Current sample rate shared with the spectrum consumer.
    pub(crate) sr_atomic: Arc<AtomicU32>,
}

/// Runs the audio engine until every command sender is dropped.
///
/// # Params:
///   - `commands`: Audio command receiver.
///   - `io`: Shared snapshot, startup, seek, and PCM handles.
///   - `mode`: Requested audio backend mode.
///   - `params`: Audio engine configuration.
pub(crate) fn run(
    commands: &mpsc::Receiver<AudioCommand>,
    io: &EngineIo,
    mode: AudioMode,
    params: &EngineParams,
) {
    if let Err(error) = engine_main(commands, io, mode, params) {
        mineral_log::error!(target: "audio", error = mineral_log::chain(&error), "engine exited");
    }
}

/// Initializes output and runs the command/snapshot loop.
fn engine_main(
    commands: &mpsc::Receiver<AudioCommand>,
    io: &EngineIo,
    mode: AudioMode,
    params: &EngineParams,
) -> color_eyre::Result<()> {
    let output = match mode {
        AudioMode::ForceNull => None,
        AudioMode::Auto => match Output::open(pct_to_gain(*params.initial_volume())) {
            Ok(output) => Some(output),
            Err(error) => {
                mineral_log::warn!(
                    target: "audio",
                    error = mineral_log::chain(&error),
                    "no audio device; running in null mode"
                );
                None
            }
        },
    };
    let Some(output) = output else {
        io.snapshot.lock().backend = AudioBackend::Null;
        let _ = io.ready_tx.send(Ok(()));
        return run_null_mode(commands);
    };
    let _ = io.ready_tx.send(Ok(()));
    let mut engine = Engine::new(output, &io.tap_producer, &io.sr_atomic);
    let tick = Duration::from_millis(*params.tick_ms());
    loop {
        match commands.recv_timeout(tick) {
            Ok(command) => engine.handle_command(command),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        engine.drain_seek(&io.seek_mailbox);
        engine.update_snapshot(&io.snapshot);
    }
    Ok(())
}

/// Drains commands without touching an audio device.
fn run_null_mode(commands: &mpsc::Receiver<AudioCommand>) -> color_eyre::Result<()> {
    while commands.recv().is_ok() {}
    Ok(())
}

/// Mutable state owned by the audio engine thread.
struct Engine {
    /// System output stream and rodio queue.
    output: Output,

    /// Shared PCM tap producer.
    tap_producer: SharedProd,

    /// Current sample rate shared with the spectrum consumer.
    sample_rate: Arc<AtomicU32>,

    /// Current and gapless-prefetched decoder accounting.
    head: PlayHead,
}

impl Engine {
    /// Creates engine state around an initialized output.
    fn new(output: Output, tap_producer: &SharedProd, sample_rate: &Arc<AtomicU32>) -> Self {
        Self {
            output,
            tap_producer: Arc::clone(tap_producer),
            sample_rate: Arc::clone(sample_rate),
            head: PlayHead::default(),
        }
    }

    /// Applies one command and contains command-local failures.
    fn handle_command(&mut self, command: AudioCommand) {
        match command {
            AudioCommand::Play { media, capture } => {
                if let Err(error) = self.play(media, capture.as_deref()) {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "play error");
                }
            }
            AudioCommand::AppendNext { media, capture } => {
                if let Err(error) = self.append_next(media, capture.as_deref()) {
                    mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "prefetch decode error");
                }
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
    fn play(
        &mut self,
        media: OpenedMedia,
        capture: Option<&std::path::Path>,
    ) -> color_eyre::Result<()> {
        let song_id = media.info().song_id.qualified();
        self.output.player().stop();
        self.head.stop();
        self.sample_rate.store(0, Ordering::Relaxed);
        let slot = self.append_opened(media, capture, "current")?;
        mineral_log::info!(target: "audio", song_id, "start decoding");
        self.output.player().play();
        self.sample_rate.store(slot.sample_rate, Ordering::Relaxed);
        self.head.start(slot);
        Ok(())
    }

    /// Appends already-opened next media behind current without reopening it.
    fn append_next(
        &mut self,
        media: OpenedMedia,
        capture: Option<&std::path::Path>,
    ) -> color_eyre::Result<()> {
        if !self.head.cur.occupied {
            media.cancellation().cancel();
            return Ok(());
        }
        self.head.clear_next();
        let slot = self.append_opened(media, capture, "next")?;
        self.head.arm_next(slot);
        Ok(())
    }

    /// Builds and appends one decoder, returning its slot accounting.
    fn append_opened(
        &mut self,
        media: OpenedMedia,
        capture: Option<&std::path::Path>,
        slot_name: &'static str,
    ) -> color_eyre::Result<Slot> {
        let byte_len = (media.seek_support() == SeekSupport::RandomAccess)
            .then_some(media.byte_len())
            .flatten();
        let transfer = media.transfer().cloned();
        let cancellation = media.cancellation().clone();
        let reader = media.into_reader();
        let (reader, capture_state) = wrap_capture(reader, capture, byte_len);
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
        self.output
            .player()
            .append(TapSource::new(source, Arc::clone(&self.tap_producer)));
        Ok(Slot {
            duration_ms,
            sample_rate,
            transfer,
            capture: capture_state,
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
    fn drain_seek(&self, mailbox: &Arc<Mutex<Option<Duration>>>) {
        let Some(target) = mailbox.lock().take() else {
            return;
        };
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
    fn update_snapshot(&mut self, snapshot: &Arc<Mutex<AudioSnapshot>>) {
        let position_ms = duration_to_ms(self.output.player().get_pos());
        let paused = self.output.player().is_paused();
        let boundary = self.head.observe(self.output.player().len());
        if boundary == Boundary::Gapless {
            self.sample_rate
                .store(self.head.cur.sample_rate, Ordering::Relaxed);
        }
        let playing = !paused && self.head.cur.occupied;
        self.output.recover_if_stalled(playing, position_ms);
        let fields = self.head.snapshot_fields();
        let mut current = snapshot.lock();
        current.playing = playing;
        current.position_ms = position_ms;
        current.duration_ms = fields.duration_ms;
        current.track_finished_seq = fields.track_finished_seq;
        current.current_track_token = fields.current_track_token;
        current.download_complete = fields.download_complete;
        current.buffered_bps = fields.buffered_bps;
        current.next_duration_ms = fields.next_duration_ms;
        current.next_buffered_bps = fields.next_buffered_bps;
        current.next_ready = fields.next_ready;
        current.next_download_complete = fields.next_download_complete;
        current.sample_rate_hz = self.head.cur.sample_rate;
    }
}

/// Wraps a prepared reader with a post-preparation capture tee when requested.
fn wrap_capture(
    reader: Box<dyn MediaReader>,
    path: Option<&std::path::Path>,
    expected_len: Option<u64>,
) -> (Box<dyn MediaReader>, Option<CaptureState>) {
    let Some(path) = path else {
        return (reader, None);
    };
    match CaptureReader::open_writer(path) {
        Ok(writer) => {
            let (reader, state) = CaptureReader::new(reader, writer, expected_len);
            (Box::new(reader), Some(state))
        }
        Err(error) => {
            mineral_log::warn!(
                target: "audio",
                error = mineral_log::chain(&error),
                "capture unavailable; continuing without capture"
            );
            (reader, None)
        }
    }
}

/// Source wrapper ending immediately after playback instance cancellation.
struct InstanceSource<S> {
    /// Decoder producing PCM samples.
    inner: S,

    /// Playback instance cancellation token.
    cancellation: CancellationToken,
}

impl<S> InstanceSource<S> {
    /// Wraps one decoder with its instance token.
    fn new(inner: S, cancellation: CancellationToken) -> Self {
        Self {
            inner,
            cancellation,
        }
    }
}

impl<S> Iterator for InstanceSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cancellation.is_cancelled() {
            None
        } else {
            self.inner.next()
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for InstanceSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        if self.cancellation.is_cancelled() {
            return Err(SeekError::NotSupported {
                underlying_source: std::any::type_name::<Self>(),
            });
        }
        self.inner.try_seek(position)
    }
}

/// Builds a rodio decoder and enables arbitrary seek only when byte length is known.
pub(crate) fn build_decoder<R>(
    reader: R,
    byte_len: Option<u64>,
) -> color_eyre::Result<rodio::Decoder<R>>
where
    R: Read + Seek + Send + Sync + 'static,
{
    let mut builder = DecoderBuilder::new().with_data(reader);
    if let Some(length) = byte_len {
        builder = builder.with_byte_len(length);
    }
    builder.build().map_err(|error| eyre!("decode: {error}"))
}

/// Converts a duration to milliseconds with saturation on impossible overflow.
fn duration_to_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

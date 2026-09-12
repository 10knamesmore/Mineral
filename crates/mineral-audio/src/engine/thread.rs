//! Dedicated audio thread startup and command polling.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::mpsc;
use std::time::Duration;

use parking_lot::Mutex;

use super::output::Output;
use super::playback::{Engine, pct_to_gain};
use crate::command::AudioCommand;
use crate::handle::{AudioMode, EngineParams};
use crate::snapshot::{AudioBackend, AudioSnapshot};
use crate::tap::SharedProd;

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

    /// 累计尝试写入 tap 的 mono 样本数(缺口辨认)。
    pub(crate) tap_pushed: Arc<AtomicU64>,
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
    let mut engine = Engine::new(output, &io.tap_producer, &io.sr_atomic, &io.tap_pushed);
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

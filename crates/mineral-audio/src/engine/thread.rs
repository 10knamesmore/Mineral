//! Dedicated audio thread startup and command polling.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use super::output::Output;
use super::playback::Engine;
use super::render::SharedPlayback;
use crate::command::{AudioCommand, SeekRequest};
use crate::handle::{AudioMode, EngineParams};
use crate::snapshot::{AudioBackend, AudioSnapshot};

/// Shared handles transferred into the dedicated audio thread.
pub(crate) struct EngineIo {
    /// Latest audio snapshot.
    pub(crate) snapshot: Arc<Mutex<AudioSnapshot>>,

    /// Latest-wins seek mailbox.
    pub(crate) seek_mailbox: Arc<Mutex<Option<SeekRequest>>>,

    /// 快照写入信号:每轮 `update_snapshot` 后唤醒等待者(发布器即时推送)。
    pub(crate) snapshot_changes: Arc<Notify>,

    /// Engine startup result channel.
    pub(crate) ready_tx: mpsc::SyncSender<crate::Result<()>>,

    /// Prepared PCM and playback state shared with workers and CPAL.
    pub(crate) playback: Arc<SharedPlayback>,
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
) -> crate::Result<()> {
    let output = match mode {
        AudioMode::ForceNull => None,
        AudioMode::Auto => Some(Output::open(Arc::clone(&io.playback))),
    };
    let Some(output) = output else {
        io.snapshot.lock().backend = AudioBackend::Null;
        let _ = io.ready_tx.send(Ok(()));
        return run_null_mode(commands);
    };
    let tick = Duration::from_millis(*params.tick_ms());
    let mut engine = Engine::new(output, Arc::clone(&io.playback), tick);
    engine.update_snapshot(&io.snapshot);
    let _ = io.ready_tx.send(Ok(()));
    loop {
        match commands.recv_timeout(tick) {
            Ok(command) => engine.handle_command(command),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        engine.drain_seek(&io.seek_mailbox);
        engine.update_snapshot(&io.snapshot);
        io.snapshot_changes.notify_one();
    }
    Ok(())
}

/// Drains commands without touching an audio device.
fn run_null_mode(commands: &mpsc::Receiver<AudioCommand>) -> crate::Result<()> {
    while let Ok(command) = commands.recv() {
        match command {
            AudioCommand::ListOutputs(reply) => {
                let _ = reply.send(Ok(Vec::new()));
            }
            AudioCommand::SelectOutput { reply, .. } => {
                let _ = reply.send(Err(crate::Error::OutputDisabled));
            }
            _ => {}
        }
    }
    Ok(())
}

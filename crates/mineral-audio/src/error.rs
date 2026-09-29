//! Audio engine startup, decoding, and output failures.

use std::path::PathBuf;

use rodio::cpal;
use thiserror::Error;

/// Failures exposed by the audio engine.
#[derive(Debug, Error)]
pub enum Error {
    /// The audio worker could not be started.
    #[error("spawn audio thread")]
    SpawnThread {
        /// Operating system thread creation failure.
        #[source]
        source: std::io::Error,
    },

    /// The audio worker exited before reporting startup.
    #[error("audio engine exited before startup")]
    StartupInterrupted {
        /// Startup channel disconnection.
        #[source]
        source: std::sync::mpsc::RecvError,
    },

    /// The audio worker no longer accepts commands.
    #[error("audio engine stopped")]
    EngineStopped,

    /// The audio worker did not answer a device request.
    #[error("audio {operation} interrupted")]
    ReplyInterrupted {
        /// Device operation that was awaiting a reply.
        operation: &'static str,

        /// Reply channel failure.
        #[source]
        source: tokio::sync::oneshot::error::RecvError,
    },

    /// Output cannot play because no device is available.
    #[error("audio output is unavailable")]
    OutputUnavailable,

    /// Device output was explicitly disabled.
    #[error("audio device output is disabled")]
    OutputDisabled,

    /// Listing output devices failed.
    #[error("list audio output devices")]
    ListDevices {
        /// CPAL device discovery failure.
        #[source]
        source: cpal::DevicesError,
    },

    /// No default output device exists.
    #[error("no default output device")]
    NoDefaultDevice,

    /// An output device identifier was invalid.
    #[error("invalid output device id {id}")]
    InvalidDeviceId {
        /// Requested identifier.
        id: String,

        /// CPAL identifier parse failure.
        #[source]
        source: cpal::DeviceIdError,
    },

    /// A requested output device is not present.
    #[error("output device is unavailable: {id}")]
    DeviceUnavailable {
        /// Requested device identifier.
        id: String,
    },

    /// Reading an output device identifier failed.
    #[error("read output device id")]
    DeviceId {
        /// CPAL device identifier failure.
        #[source]
        source: cpal::DeviceIdError,
    },

    /// Reading an output device description failed.
    #[error("read output device description")]
    DeviceName {
        /// CPAL device description failure.
        #[source]
        source: cpal::DeviceNameError,
    },

    /// Reading the default output format failed.
    #[error("read default audio output configuration")]
    OutputConfig {
        /// CPAL configuration failure.
        #[source]
        source: cpal::DefaultStreamConfigError,
    },

    /// A device supplied an invalid channel count.
    #[error("output config has zero channels")]
    ZeroChannels,

    /// A device supplied an invalid sample rate.
    #[error("output config has zero sample rate")]
    ZeroSampleRate,

    /// A device supplied a sample format the output cannot render.
    #[error("unsupported output sample format: {format:?}")]
    UnsupportedSampleFormat {
        /// CPAL sample format supplied by the device.
        format: cpal::SampleFormat,
    },

    /// Building an output stream failed.
    #[error("build audio output stream")]
    BuildStream {
        /// CPAL stream construction failure.
        #[source]
        source: cpal::BuildStreamError,
    },

    /// Starting an output stream failed.
    #[error("start audio output stream")]
    PlayStream {
        /// CPAL stream playback failure.
        #[source]
        source: cpal::PlayStreamError,
    },

    /// Pausing an output stream failed.
    #[error("pause audio output stream")]
    PauseStream {
        /// CPAL stream pause failure.
        #[source]
        source: cpal::PauseStreamError,
    },

    /// The encoded media could not be decoded.
    #[error("decode media")]
    Decode {
        /// Rodio decoder failure.
        #[source]
        source: rodio::decoder::DecoderError,
    },

    /// Reading an audio file failed.
    #[error("open audio file {}", path.display())]
    OpenFile {
        /// Input audio file path.
        path: PathBuf,

        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },

    /// No complete frame could be decoded from an audio file.
    #[error("no complete audio frames in {}", path.display())]
    NoFrames {
        /// Input audio file path.
        path: PathBuf,
    },
}

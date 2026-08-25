//! Decoder-ready encoded media and its open-time execution options.

use std::io::{Read, Seek};

use mineral_model::PlaybackMediaInfo;
use tokio_util::sync::CancellationToken;

use super::TransferState;

/// Object-safe synchronous decoder input facade.
pub trait MediaReader: Read + Seek + Send + Sync {}

impl<T: Read + Seek + Send + Sync> MediaReader for T {}

/// The random-access capability of opened encoded media.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekSupport {
    /// The reader may only advance from its current position.
    ForwardOnly,

    /// The reader supports arbitrary absolute access.
    RandomAccess,
}

/// Execution controls supplied when consuming a prepared playback.
#[non_exhaustive]
#[derive(Clone)]
pub struct OpenOptions {
    /// Cancellation token owned by the playback instance.
    cancellation: CancellationToken,

    /// Bytes the built-in streaming opener should prepare before returning.
    prefetch_bytes: u64,
}

impl OpenOptions {
    /// Creates open-time execution controls.
    ///
    /// # Params:
    ///   - `cancellation`: Token owned by the playback instance.
    ///   - `prefetch_bytes`: Bytes to prepare before returning a streaming reader.
    #[must_use]
    pub fn new(cancellation: CancellationToken, prefetch_bytes: u64) -> Self {
        Self {
            cancellation,
            prefetch_bytes,
        }
    }

    /// Returns the playback instance cancellation token.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Returns the requested streaming prefetch amount.
    #[must_use]
    pub fn prefetch_bytes(&self) -> u64 {
        self.prefetch_bytes
    }
}

/// Decoder-ready encoded media returned by a prepared playback.
pub struct OpenedMedia {
    /// Synchronous decoder-facing reader.
    reader: Box<dyn MediaReader>,

    /// Random-access capability of the reader.
    seek_support: SeekSupport,

    /// Encoded byte length when known.
    byte_len: Option<u64>,

    /// Final source-neutral media facts.
    info: PlaybackMediaInfo,

    /// Streaming transfer progress when a producer is involved.
    transfer: Option<TransferState>,

    /// Cancellation token governing reader lifetime.
    cancellation: CancellationToken,
}

impl OpenedMedia {
    /// Creates decoder-ready media.
    ///
    /// # Params:
    ///   - `reader`: Synchronous encoded-media reader.
    ///   - `seek_support`: Reader random-access capability.
    ///   - `byte_len`: Encoded byte length when known.
    ///   - `info`: Final source-neutral media facts.
    ///   - `transfer`: Optional streaming transfer progress.
    ///   - `cancellation`: Token governing the reader lifetime.
    #[must_use]
    pub fn new(
        reader: Box<dyn MediaReader>,
        seek_support: SeekSupport,
        byte_len: Option<u64>,
        info: PlaybackMediaInfo,
        transfer: Option<TransferState>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            reader,
            seek_support,
            byte_len,
            info,
            transfer,
            cancellation,
        }
    }

    /// Returns the reader access capability.
    #[must_use]
    pub fn seek_support(&self) -> SeekSupport {
        self.seek_support
    }

    /// Returns the encoded byte length when known.
    #[must_use]
    pub fn byte_len(&self) -> Option<u64> {
        self.byte_len
    }

    /// Returns the final source-neutral media facts.
    #[must_use]
    pub fn info(&self) -> &PlaybackMediaInfo {
        &self.info
    }

    /// Returns streaming transfer progress when present.
    #[must_use]
    pub fn transfer(&self) -> Option<&TransferState> {
        self.transfer.as_ref()
    }

    /// Returns the token governing the reader lifetime.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Consumes the media and returns its decoder-facing reader.
    #[must_use]
    pub fn into_reader(self) -> Box<dyn MediaReader> {
        self.reader
    }
}

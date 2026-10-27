//! Decoder-ready encoded media and its open-time execution options.

use std::io::{Read, Seek};

use mineral_model::PlaybackMediaInfo;
use tokio_util::sync::CancellationToken;

use super::{
    AudioInput, AudioPacketSource, CaptureReceipt, CaptureTarget, ReadControl, TransferState,
};

/// Synchronous encoded-file access supporting random byte access.
pub trait MediaReader: Read + Seek + Send + Sync {}

impl<T: Read + Seek + Send + Sync> MediaReader for T {}

/// Execution controls supplied when consuming a prepared playback.
///
/// A capture target requests a complete producer-owned copy of the prepared encoded media. The
/// provider reports its terminal result through [`OpenedMedia::take_capture`]; decoder reads never
/// define capture completion.
#[non_exhaustive]
#[derive(Clone)]
pub struct OpenOptions {
    /// Cancellation token owned by the playback instance.
    cancellation: CancellationToken,

    /// Bytes the built-in streaming opener should prepare before returning.
    prefetch_bytes: u64,

    /// Optional destination for a complete copy of the prepared encoded media.
    capture_target: Option<CaptureTarget>,
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
            capture_target: None,
        }
    }

    /// Requests producer-owned capture into a unique temporary path.
    ///
    /// # Params:
    ///   - `target`: Destination for the complete prepared encoded media.
    #[must_use]
    pub fn capture_to(mut self, target: CaptureTarget) -> Self {
        self.capture_target = Some(target);
        self
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

    /// Returns the requested capture target.
    #[must_use]
    pub fn capture_target(&self) -> Option<&CaptureTarget> {
        self.capture_target.as_ref()
    }
}

/// Decoder-ready media with independent access to a complete encoded file.
pub struct OpenedMedia {
    /// Complete encoded-file access for playback or export; reads may wait for acquisition.
    reader: Box<dyn MediaReader>,

    /// Optional progressive packet delivery, independent of complete-file materialization.
    audio_packets: Option<Box<dyn AudioPacketSource>>,

    /// Encoded byte length when known.
    byte_len: Option<u64>,

    /// Final source-neutral media facts.
    info: PlaybackMediaInfo,

    /// Streaming transfer progress when a producer is involved.
    transfer: Option<TransferState>,

    /// Read demand that the decoder worker can interrupt on seek.
    read_control: Option<ReadControl>,

    /// Producer-owned completion receipt for the requested capture.
    capture: Option<CaptureReceipt>,

    /// Cancellation token governing reader lifetime.
    cancellation: CancellationToken,
}

impl OpenedMedia {
    /// Creates decoder-ready media.
    ///
    /// # Params:
    ///   - `reader`: Synchronous encoded-media reader.
    ///   - `byte_len`: Encoded byte length when known.
    ///   - `info`: Final source-neutral media facts.
    ///   - `transfer`: Optional streaming transfer progress.
    ///   - `cancellation`: Token governing the reader lifetime.
    #[must_use]
    pub fn new(
        reader: Box<dyn MediaReader>,
        byte_len: Option<u64>,
        info: PlaybackMediaInfo,
        transfer: Option<TransferState>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            reader,
            audio_packets: None,
            byte_len,
            info,
            transfer,
            read_control: None,
            capture: None,
            cancellation,
        }
    }

    /// Uses source-prepared packets for playback while retaining encoded-file access for export.
    /// This lets segmented sources start decoding before a complete file can be materialized.
    #[must_use]
    pub fn with_audio_packets(mut self, packets: Box<dyn AudioPacketSource>) -> Self {
        self.audio_packets = Some(packets);
        self
    }

    /// Attaches read demand shared with the provider's reader.
    ///
    /// # Params:
    ///   - `control`: Demand governed by this media's reader lifetime token.
    #[must_use]
    pub fn with_read_control(mut self, control: ReadControl) -> Self {
        self.read_control = Some(control);
        self
    }

    /// Attaches the producer-owned completion receipt for a requested capture.
    ///
    /// # Params:
    ///   - `capture`: Receipt that verifies the complete prepared media file.
    #[must_use]
    pub fn with_capture(mut self, capture: CaptureReceipt) -> Self {
        self.capture = Some(capture);
        self
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

    /// Returns read demand that can be interrupted after decoder initialization.
    #[must_use]
    pub fn read_control(&self) -> Option<&ReadControl> {
        self.read_control.as_ref()
    }

    /// Takes the producer-owned capture receipt, if capture was requested and accepted.
    pub fn take_capture(&mut self) -> Option<CaptureReceipt> {
        self.capture.take()
    }

    /// Returns the token governing the reader lifetime.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Consumes the media and returns complete encoded-file access for export.
    #[must_use]
    pub fn into_reader(self) -> Box<dyn MediaReader> {
        self.reader
    }

    /// Consumes the media and selects its playback input independently of file export.
    #[must_use]
    pub fn into_audio_input(self) -> AudioInput {
        match self.audio_packets {
            Some(packets) => AudioInput::Packets(packets),
            None => AudioInput::Bytes {
                reader: self.reader,
                byte_len: self.byte_len,
            },
        }
    }
}

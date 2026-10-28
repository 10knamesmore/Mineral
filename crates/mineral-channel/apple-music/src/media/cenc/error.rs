use shiguredo_mp4::Error as Mp4Error;
use thiserror::Error;

pub(super) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("MP4 decoding failed: {0}")]
    Mp4(#[from] Mp4Error),

    #[error("missing box {0:?}")]
    MissingBox([u8; 4]),

    #[error("duplicate box {0:?}")]
    DuplicateBox([u8; 4]),

    #[error("truncated {box_type:?}: need {needed} bytes, have {remaining}")]
    Truncated {
        box_type: [u8; 4],

        needed: usize,

        remaining: usize,
    },

    #[error("invalid {box_type:?} field {field}")]
    InvalidField {
        box_type: [u8; 4],

        field: &'static str,
    },

    #[error("unsupported {box_type:?} version {version} or flags {flags:#x}")]
    BoxVersionOrFlags {
        box_type: [u8; 4],

        version: u8,

        flags: u32,
    },

    #[error("unsupported encryption scheme {0:?}")]
    Scheme([u8; 4]),

    #[error("unsupported form: {0:?}")]
    Unsupported(Unsupported),

    #[error("content key KID differs from tenc KID")]
    KeyIdMismatch,

    #[error("sample description index {index} is outside the {count} initialization descriptions")]
    SampleDescriptionIndex {
        /// One-based description index from tfhd or trex.
        index: u32,

        /// Number of stsd entries in the initialization.
        count: usize,
    },

    #[error("{box_type:?} sample count {actual} differs from trun count {expected}")]
    SampleCount {
        box_type: [u8; 4],

        expected: usize,

        actual: usize,
    },

    #[error("sample {sample} range {offset}..+{size} is not inside an mdat payload")]
    SampleOutsideMedia {
        sample: usize,

        offset: u64,

        size: usize,
    },

    #[error("sample {sample} subsample ranges cover {covered} bytes, sample has {size}")]
    SubsampleCoverage {
        sample: usize,

        covered: usize,

        size: usize,
    },

    #[error("auxiliary data disagrees with senc for sample {sample}")]
    AuxiliaryMismatch { sample: usize },

    #[error("offset, size, or timestamp arithmetic overflow")]
    ArithmeticOverflow,

    #[error("AES-CTR counter exhausted")]
    CounterExhausted,

    #[error("CBCS protected data is not aligned to AES blocks")]
    CbcBlockAlignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unsupported {
    MultipleTracks,

    NonAudioTrack,

    AudioConfigurationChange,

    MultipleContentKeys,

    MissingContentDescription,

    NonEncryptedAudioEntry,

    NonAacEntry,

    QuickTimeAudioEntry,

    ExternalDataReference,

    PatternEncryption,

    ConstantIv,

    UnprotectedTrack,

    IvSize(u8),

    SampleEncryptionOverride,

    SampleEncryptionGroups,

    UuidEncryptionBox,

    ProtectionBox([u8; 4]),

    MultipleMovieFragments,

    MultipleTrackFragments,

    EmptyFragment,

    AuxiliaryLayout,

    AuxiliaryType,

    NonFragmentedInitialization,
}

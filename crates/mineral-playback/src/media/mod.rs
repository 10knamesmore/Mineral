//! Decoder-ready media values and optional direct access descriptions.

mod opened;
mod transfer;

pub use opened::{MediaReader, OpenOptions, OpenedMedia, SeekSupport};
pub use transfer::{TransferSnapshot, TransferState};

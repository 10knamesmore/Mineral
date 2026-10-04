//! 曲目列表的状态与呈现。

mod paint;
mod state;
mod view;

pub use state::TrackList;
pub(crate) use view::{TrackInput, TrackView};

mod input;

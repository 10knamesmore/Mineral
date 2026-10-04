//! 歌单列表的状态与呈现。

mod paint;
mod state;
mod view;

pub use state::PlaylistList;
pub(crate) use view::{PlaylistInput, PlaylistView};

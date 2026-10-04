//! 歌词窗口绘制与布局转场。

mod morph;
mod panel;
mod state;
mod sweep;
mod view;
pub use state::LyricsPanel;
pub(crate) use view::{LyricsInput, LyricsView};

pub(crate) use morph::draw_transition;
pub use panel::LyricMode;
pub(crate) use panel::draw;
pub(crate) use sweep::LyricColors;

mod glide;

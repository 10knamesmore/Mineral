//! 歌词窗口绘制与布局转场。

mod morph;
mod panel;
mod sweep;

pub(crate) use morph::draw_transition;
pub(crate) use panel::prepare;
pub use panel::{LyricMode, draw};
pub(crate) use sweep::LyricColors;

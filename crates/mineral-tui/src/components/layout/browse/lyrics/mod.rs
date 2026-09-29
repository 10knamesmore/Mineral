//! 歌词窗口绘制与布局转场。

mod morph;
mod panel;

pub(crate) use morph::draw_transition;
pub use panel::{LyricMode, draw};

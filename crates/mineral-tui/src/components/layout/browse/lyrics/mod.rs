//! 歌词窗口绘制与页面之间的当前行移动。

mod morph;
mod panel;

pub(crate) use morph::LyricTransition;
pub use panel::{LyricMode, draw};

#[cfg(test)]
mod morph_tests;

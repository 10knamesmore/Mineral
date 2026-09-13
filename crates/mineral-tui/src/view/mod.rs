//! 主帧绘制与页面内容转场。

mod frame;
mod page_morph;

pub use frame::draw;

#[cfg(test)]
mod page_morph_tests;

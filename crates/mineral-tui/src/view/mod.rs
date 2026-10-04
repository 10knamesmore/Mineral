//! 主帧绘制与页面内容转场。

mod frame;
mod page_morph;
mod preparation;

pub(crate) use frame::draw;

#[cfg(test)]
mod page_morph_tests;

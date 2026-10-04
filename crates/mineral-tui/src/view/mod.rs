//! 主帧绘制与页面内容转场。

mod frame;
mod page_morph;
mod preparation;

pub(crate) use frame::draw;

#[cfg(test)]
mod page_morph_tests;

pub(crate) mod browse;

mod transport;

pub(crate) mod lyrics;

pub(crate) mod now_playing;

pub(crate) mod flight;

mod status;

pub(crate) mod search;

mod root;
pub(crate) use root::RootView;

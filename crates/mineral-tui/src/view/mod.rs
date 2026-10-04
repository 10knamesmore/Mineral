//! 主帧绘制与页面内容转场。

mod frame;
mod layers;
mod page_morph;
mod preparation;

#[cfg(test)]
pub(crate) use frame::draw;
pub(crate) use frame::plan;

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

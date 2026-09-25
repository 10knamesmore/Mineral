//! 播放栏的五行绘制与本地操作反馈。

mod feedback;
mod paint;

pub(crate) use feedback::TransportFeedback;
pub(crate) use paint::{draw, split_buffered_track};

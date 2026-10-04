//! 播放栏的五行绘制与本地操作反馈。

mod feedback;
mod paint;
mod progress;
mod view;
mod waveform;
pub(crate) use view::{TransportInput, TransportView};

pub(crate) use feedback::TransportBar;

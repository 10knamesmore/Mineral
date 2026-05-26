//! macOS 系统 Now Playing 后端:经 MPNowPlayingInfoCenter 上报曲目、MPRemoteCommandCenter
//! 收播放控制 / 媒体键,在 Control Center 显示并可控。
//!
//! 状态更新由 [`MediaService`] 投递给专属线程应用;命令在主线程 run loop 派发,主线程
//! 的 NSApplication + run loop 由 [`macos_init_app`] / [`macos_pump_until`] 驱动。

mod app;
mod command;
mod convert;
mod now_playing;
mod service;

pub use app::{MacApp, macos_init_app, macos_pump_until};
pub use service::MediaService;

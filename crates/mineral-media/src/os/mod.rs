//! 平台相关的 [`MediaService`] 后端:按 `target_os` 选择具体实现。
//!
//! - Linux:`os::linux`(MPRIS)。
//! - macOS:`os::macos`(系统 Now Playing:MPNowPlayingInfoCenter / MPRemoteCommandCenter)。
//! - 其它平台:`os::stub`(no-op 占位)。

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::MediaService;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{MacApp, MediaService, macos_init_app, macos_pump_until};

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod stub;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use stub::MediaService;

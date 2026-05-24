//! 平台相关的 [`MediaService`] 后端:按 `target_os` 选择具体实现。
//!
//! - Linux:`os::linux`(MPRIS via mpris-server)。
//! - 其它平台(macOS 等):`os::macos`(目前是 no-op 占位)。

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::MediaService;

#[cfg(not(target_os = "linux"))]
mod macos;

#[cfg(not(target_os = "linux"))]
pub use macos::MediaService;

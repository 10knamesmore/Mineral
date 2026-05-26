//! 系统媒体服务集成的平台无关封装。
//!
//! 把「系统媒体控件 ↔ 播放器」的对接收敛成一组平台无关类型:上报曲目元数据
//! ([`NowPlaying`])与播放状态([`PlaybackState`]),接收来自系统的控制命令
//! ([`MediaCommand`])。
//!
//! 后端按平台选择:
//! - **Linux**:MPRIS(`org.mpris.MediaPlayer2.*`)。
//! - **macOS**:系统 Now Playing(MPNowPlayingInfoCenter / MPRemoteCommandCenter)。
//! - **其它平台**:no-op stub(编译通过但不做事)。

mod command;
mod config;
mod os;
mod state;

pub use command::{LoopMode, MediaCommand};
pub use config::MediaConfig;
pub use os::MediaService;
pub use state::{NowPlaying, PlaybackState};

/// macOS 专属:主线程 NSApplication 句柄与 run loop 驱动入口。
///
/// 系统媒体中心只把命令派发到**主线程的 run loop**,且未打包二进制需主线程养起
/// NSApplication 才会被收录。故 daemon 在主线程调 [`macos_init_app`] 起 NSApplication、
/// 再调 [`macos_pump_until`] 阻塞 pump,后台线程跑 tokio。
#[cfg(target_os = "macos")]
pub use os::{MacApp, macos_init_app, macos_pump_until};

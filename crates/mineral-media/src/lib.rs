//! 系统媒体服务集成的平台无关封装。
//!
//! 把「系统媒体控件 ↔ 播放器」的对接收敛成一组平台无关类型:上报曲目元数据
//! ([`NowPlaying`])与播放状态([`PlaybackState`]),接收来自系统的控制命令
//! ([`MediaCommand`])。
//!
//! 后端按平台选择:
//! - **Linux**:MPRIS(`org.mpris.MediaPlayer2.*`),基于 souvlaki 的 zbus 后端。
//! - **其它平台**:目前是 no-op stub(编译通过但不做事)。

mod command;
mod config;
mod os;
mod state;

pub use command::MediaCommand;
pub use config::MediaConfig;
pub use os::MediaService;
pub use state::{NowPlaying, PlaybackState};

//! 服务端播放上下文、起播与队列控制，以及独立于 client 的自动续播和会话维护。

mod context;
mod control;
mod download_control;
mod maintenance;
mod song_playback;
mod startup;
mod transport;

pub(crate) use context::Inner;
pub use context::PlayerCore;
pub(crate) use startup::{PlayerStorage, Sinks, SpawnConfig};

#[cfg(test)]
mod tests;

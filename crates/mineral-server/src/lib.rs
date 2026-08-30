//! 承载 Mineral 的后台播放、取数、任务调度与 client 接入。
//!
//! [`Server`] 持有进程内资源,可通过 [`Server::client`] 创建本地 [`ClientHandle`],
//! 也可通过 [`Server::serve`] 在 Unix socket 上接入独立 client。
//!
//! ## 角色边界
//!
//! - **Server**:拥有 audio engine、task worker、channels、playback providers 与事件 hub,
//!   并提供本地 handle、IPC accept loop 与关停入口。
//! - **ClientHandle**:`Clone`,只暴露命令、snapshot 与事件接口,不泄漏 server 内部句柄。

mod client;
mod config;
mod config_host;
mod download;
mod envelope;
mod events;
mod favorites;
mod gapless;
mod hook_bridge;
mod library;
mod media;
mod media_cache;
mod notify;
mod pcm;
mod playback;
mod playback_instance;
mod player;
mod props;
mod queue;
mod resolve;
mod script_bridge;
mod script_reload;
mod serve;
mod server;
mod session;
mod state;
mod stats;
mod tagging;

pub use client::{Client, ClientHandle};
pub use config::{ServerConfig, resolve_audio_mode};
pub use mineral_audio::AudioMode;
pub use mineral_protocol::ChannelFetchKindTag;
pub use script_bridge::{ScriptParts, ScriptPumps, ScriptReloadParts};
pub use script_reload::spawn_script_reloader;
pub use server::{Server, SourceBackends};
pub use stats::{
    PendingPlay, StatsRecorder, now_ms, params_from_config, pending_from_start, stats_play_mode,
};

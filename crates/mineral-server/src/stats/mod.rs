//! 埋点采集(server 侧):config 折算([`config`])+ recorder writer-actor([`recorder`])+
//! 编译期埋点遗忘防线([`audit`])。

mod audit;
mod config;
mod recorder;

pub use config::params_from_config;
pub use recorder::{PendingPlay, StatsRecorder, now_ms, pending_from_start, stats_play_mode};

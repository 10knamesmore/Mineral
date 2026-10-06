//! daemon 音频、来源和服务配置,以及本宿主 Lua 资产与加载入口。

mod callbacks;
mod check;
mod init;
mod loader;
mod lua_stub;
mod schema;
mod session;
mod startup;

pub use check::render_check;
pub use init::init;
pub(crate) use loader::daemon_from_tree;
#[cfg(test)]
pub(crate) use loader::default_daemon_tree;
pub use loader::{DaemonLoad, load_daemon, load_daemon_with_vm};
pub use schema::{
    BackendKind, BilibiliSection, DaemonConfig, NeteaseSection, SourcesConfig, StatsLevel,
};
pub(crate) use schema::{
    DownloadConfig, EnvelopeConfig, RetentionDays, SearchQueryMode, StatsConfig,
};
pub(crate) use session::ConfigHost;
pub use startup::{ServerConfig, resolve_audio_mode};

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod tests;

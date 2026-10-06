//! 本宿主配置根与全部子段;只由 config 模块组装。

mod audio;
mod cache;
mod download;
mod envelope;
mod queue;
mod root;
mod script;
mod sources;
mod stats;

pub use audio::{AudioConfig, BackendKind};
pub use cache::CacheConfig;
pub use download::DownloadConfig;
pub use envelope::{EnvelopeConfig, HighpassConfig, ShelfConfig};
pub use queue::{QueueConfig, QueueTransform};
pub use root::DaemonConfig;
pub use script::ScriptConfig;
pub use sources::{
    BackfillSection, BilibiliSection, LocalSection, MineralSection, NeteaseSection,
    PlaylistFetchSection, SourcesConfig,
};
pub use stats::{ReportConfig, RetentionDays, SearchQueryMode, StatsConfig, StatsLevel};

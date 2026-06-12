//! 强类型配置 schema:顶层 [`Config`] 与各域子段。

mod animation;
mod audio;
mod behavior;
mod cache;
mod config;
mod copy;
mod cover;
mod daemon;
mod download;
mod keys;
mod layout;
mod lyrics;
mod prefetch;
mod script;
mod search;
mod sources;
mod spectrum;
mod theme;
mod toast;

pub use animation::{AnimationConfig, SweepStyle};
pub use audio::{AudioConfig, BackendKind};
pub use behavior::{BehaviorConfig, TrackPosMemory};
pub use cache::CacheConfig;
pub use config::{Config, TuiConfig};
pub use copy::{COPY_TEMPLATE_FNS, CopyConfig, CopyContext, CopyTemplate};
pub use cover::{CoverConfig, CoverStorageMode, KmeansConfig};
pub use daemon::DaemonConfig;
pub use download::DownloadConfig;
pub use keys::{KeyBinding, KeysConfig};
pub use layout::LayoutConfig;
pub use lyrics::LyricsConfig;
pub use prefetch::PrefetchConfig;
pub use script::ScriptConfig;
pub use search::{DeepWeights, SearchConfig};
pub use sources::{NeteaseSection, SourcesConfig};
pub use spectrum::SpectrumConfig;
pub use theme::{
    ColorRef, HexColor, RolesConfig, SearchHitConfig, TextStyle, ThemeConfig, TokenName,
};
pub use toast::ToastConfig;

//! 强类型配置 schema:顶层 [`Config`] 与各域子段。

mod ambient;
mod animation;
mod audio;
mod behavior;
mod cache;
mod config;
mod copy;
mod cover;
mod daemon;
mod de;
mod download;
mod envelope;
mod keys;
mod layout;
mod lyrics;
mod prefetch;
mod script;
mod search;
mod sources;
mod spectrum;
mod stats;
mod theme;
mod toast;
mod waveform;
mod window_title;

pub use ambient::{
    AmbientConfig, AnchorConfig, DriftConfig, PulseConfig, PulseDepthConfig, PunchConfig,
    RotateConfig, VignetteConfig,
};
pub use animation::{
    AnimationConfig, MarqueeBounceConfig, MarqueeConfig, MarqueeLoopConfig, MarqueeMode,
    MenuReveal, SearchFocusTransition, SweepStyle,
};
pub use audio::{AudioConfig, BackendKind};
pub use behavior::{BehaviorConfig, TrackPosMemory};
pub use cache::CacheConfig;
pub use config::{Config, TuiConfig};
pub use copy::{COPY_TEMPLATE_FNS, CopyConfig, CopyContext, CopyTemplate};
pub use cover::{
    CoverCacheConfig, CoverConfig, CoverProtocolMode, CoverStorageMode, CoverTransitionConfig,
    CoverTransitionStyle, KittyTransmitConfig, KmeansConfig, ZoomConfig,
};
pub use daemon::DaemonConfig;
pub use download::DownloadConfig;
pub use envelope::{EnvelopeConfig, HighpassConfig, ShelfConfig};
pub use keys::{KeyBinding, KeysConfig};
pub use layout::{LayoutConfig, MenuAlign};
pub use lyrics::LyricsConfig;
pub use prefetch::PrefetchConfig;
pub use script::ScriptConfig;
pub use search::{ChannelSearchConfig, DeepSearchConfig, DeepWeights, SearchConfig};
pub use sources::{
    BackfillSection, BilibiliSection, CURATE_PLAYLISTS_MERGED_FN, CURATE_PLAYLISTS_SOURCE_FNS,
    MineralSection, NeteaseSection, SourcesConfig,
};
pub use spectrum::{
    BarsConfig, ScopeConfig, SpectrumConfig, SpectrumStyle, TerrainConfig, WaterfallConfig,
};
pub use stats::{ReportConfig, RetentionDays, SearchQueryMode, StatsConfig, StatsLevel};
pub use theme::{
    AnsiSlot, ColorRef, ColorValue, DynamicThemeConfig, HexColor, SearchHitConfig, TextAlphaConfig,
    TextStyle, ThemeConfig, TokenName,
};
pub use toast::ToastConfig;
pub use waveform::WaveformConfig;
pub use window_title::{
    TimeFormat, TimePreset, TitleField, TitleIcons, TitleSegment, WindowTitleConfig,
};

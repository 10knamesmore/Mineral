//! 本宿主配置根与全部子段;只由 config 模块组装。

mod ambient;
mod animation;
mod behavior;
mod copy;
mod cover;
mod keys;
mod layout;
mod lyrics;
mod minimap;
mod prefetch;
mod progress;
mod root;
mod script;
mod search;
mod spectrum;
mod theme;
mod toast;
mod waveform;
mod window_title;

pub use ambient::{
    AmbientConfig, AnchorConfig, DriftConfig, PulseConfig, PulseDepthConfig, PunchConfig,
    RotateConfig, VignetteConfig,
};
pub use animation::{
    AmbientTrailConfig, AnimationConfig, MarqueeBounceConfig, MarqueeConfig, MarqueeLoopConfig,
    MarqueeMode, MenuReveal, SearchFocusTransition, SweepStyle, TrailTimingConfig,
    TransportFeedbackConfig,
};
pub use behavior::{BehaviorConfig, FilterPlayScope, TrackPosMemory};
pub use copy::{CopyConfig, CopyContext, CopyTemplate};
pub use cover::{
    CoverCacheConfig, CoverCellFit, CoverConfig, CoverDecodePixelsConfig, CoverProtocolMode,
    CoverTransitionConfig, CoverTransitionStyle, KmeansConfig, ZoomConfig,
};
pub use keys::{KeyBinding, KeysConfig};
pub use layout::{FsSpectrumConfig, LayoutConfig, MenuAlign};
pub use lyrics::{LyricTextAlphaConfig, LyricsConfig};
pub use minimap::MinimapConfig;
pub use prefetch::PrefetchConfig;
pub use progress::{PlayheadConfig, ProgressConfig, ProgressTrackConfig};
pub use root::TuiConfig;
pub use script::TuiScriptConfig;
pub use search::{ChannelSearchConfig, DeepSearchConfig, DeepWeights, SearchConfig};
pub use spectrum::{
    BarsConfig, ScopeConfig, SpectrumConfig, SpectrumStyle, TerrainConfig, WaterfallConfig,
};
pub use theme::{
    AnsiSlot, ColorRef, ColorValue, DynamicThemeConfig, SearchHitConfig, TextAlphaConfig,
    TextStyle, ThemeConfig,
};
pub use toast::ToastConfig;
pub use waveform::{RevealConfig, WaveformConfig};
pub use window_title::{TimeFormat, TitleField, TitleIcons, TitleSegment, WindowTitleConfig};

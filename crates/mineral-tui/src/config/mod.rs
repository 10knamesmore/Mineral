//! 当前 TUI 的全部配置 schema、Lua 资产、加载与本地执行入口。

mod callbacks;
mod check;
mod evaluator;
mod init;
pub(crate) mod key_syntax;
mod loader;
mod lua_stub;
mod schema;

pub use check::render_check;
pub(crate) use evaluator::{Error as LoadError, evaluate_tui};
pub use init::init;
pub use loader::load_tui;
pub(crate) use loader::{default_tui_tree, tui_from_tree};
pub use schema::TuiConfig;
pub(crate) use schema::{
    AmbientConfig, AnchorConfig, AnimationConfig, AnsiSlot, BehaviorConfig, ChannelSearchConfig,
    ColorRef, ColorValue, CopyContext, CopyTemplate, CoverCellFit, CoverConfig,
    CoverDecodePixelsConfig, CoverProtocolMode, CoverTransitionStyle, DeepWeights, KeyBinding,
    KeysConfig, KmeansConfig, LayoutConfig, LyricTextAlphaConfig, LyricsConfig, MarqueeConfig,
    MarqueeMode, MenuAlign, MenuReveal, MinimapConfig, ProgressConfig, PulseConfig,
    SearchFocusTransition, SpectrumConfig, SpectrumStyle, SweepStyle, TextStyle, ThemeConfig,
    TimeFormat, TitleField, TitleSegment, TrailTimingConfig, TuiScriptConfig, WaveformConfig,
    WindowTitleConfig,
};

#[cfg(test)]
mod evaluator_tests;
#[cfg(test)]
mod tests;

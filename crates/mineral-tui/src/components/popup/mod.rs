//! 浮层 / modal 子模块。
//!
//! 统一的 [`Overlay`] 基础组件(chrome 自动提供居中 layout + 弹出动画)+ [`OverlayStack`]
//! 栈管理。全部用 [`Flex::Center`] 思路的百分比 + min/max clamp 计算位置,不写死字符尺寸。
//!
//! [`Overlay`]: component::Overlay
//! [`OverlayStack`]: stack::OverlayStack

mod audio_settings;
mod component;
mod confirm;
mod disconnect;
mod download;
mod help;
mod menu;
mod placement;
mod queue;
mod stack;

pub(crate) use audio_settings::AudioSettingsOverlay;
pub(crate) use component::{
    Chrome, Overlay, OverlayAction, OverlayEnv, OverlayResponse, dock_full_rect, render_overlay,
};
pub(crate) use confirm::ConfirmOverlay;
pub(crate) use disconnect::DisconnectOverlay;
pub(crate) use download::{DownloadInput, DownloadOverlay};
pub(crate) use help::HelpOverlay;
pub(crate) use help::chip_text;
pub(crate) use menu::{ContainerRef, MenuAction, MenuItem, PopMenu};
pub(crate) use placement::Placement;
pub(crate) use queue::{QueueInput, QueueOverlay};
pub(crate) use stack::OverlayStack;

mod text_prompt;
pub(crate) use text_prompt::TextPrompt;

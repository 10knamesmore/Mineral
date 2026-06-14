//! 浮层 / modal 子模块。
//!
//! 统一的 [`Overlay`] 基础组件(chrome 自动提供居中 layout + 弹出动画)+ [`OverlayStack`]
//! 栈管理。全部用 [`Flex::Center`] 思路的百分比 + min/max clamp 计算位置,不写死字符尺寸。
//!
//! [`Overlay`]: component::Overlay
//! [`OverlayStack`]: stack::OverlayStack

mod component;
mod confirm;
mod disconnect;
mod menu;
mod placement;
mod queue;
mod stack;

pub(crate) use component::{OverlayAction, OverlayResponse, render_overlay};
pub(crate) use menu::{MenuAction, MenuItem, PopMenu};
pub(crate) use placement::Placement;
pub(crate) use stack::{OverlayKind, OverlayStack};

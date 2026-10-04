//! Spectrum 频谱面板(模块组织)。

mod render;
mod state;

pub use render::draw;
pub(crate) use render::prepare;
pub use state::SpectrumState;

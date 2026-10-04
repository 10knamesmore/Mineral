//! 应用选择哪些浮层，以及为它们接入哪些业务数据。

mod content;
pub(crate) mod input;
mod lifecycle;

pub(crate) use content::AppOverlay;

//! 复制动作、结果提示与独占系统剪贴板句柄的后台线程。

mod actions;
mod worker;

pub(crate) use actions::ClipboardState;

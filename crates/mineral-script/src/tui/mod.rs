//! TUI 独立配置加载、setup 效果收集与本地复制模板的 VM 生命周期。

mod command;
mod evaluator;
mod runtime;

pub use command::TuiCommand;
pub use evaluator::{TuiLoad, evaluate_tui};
pub(crate) use runtime::TuiHost;
pub use runtime::TuiRuntime;

//! Mineral 顶层 CLI 分发。

mod core;
mod subcommands;

pub use crate::core::{Args, Command, run};
pub use crate::subcommands::serve::run as serve_run;

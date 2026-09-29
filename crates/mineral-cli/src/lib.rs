//! Mineral 顶层 CLI 分发。

mod core;
mod error;
mod subcommands;

pub use error::{Error, Result};
pub use subcommands::stats::window::WindowError;

pub use crate::core::{Args, Command, run};
pub use crate::subcommands::channel::{bilibili_config_from, netease_config_from};
pub use crate::subcommands::serve::run as serve_run;

//! Mineral 顶层 CLI 分发。

mod core;
mod subcommands;

pub use crate::core::{run, Args, Command};

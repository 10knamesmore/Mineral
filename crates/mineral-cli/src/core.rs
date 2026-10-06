//! 顶层 CLI 类型与运行入口。

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tokio::runtime::Runtime;

use crate::error::{Error, Result};

use crate::subcommands::cache::{self, CacheCommand};
use crate::subcommands::channel::{self, ChannelArgs};
use crate::subcommands::config::{self, ConfigCommand};
use crate::subcommands::ctl::{self, CtlArgs};
use crate::subcommands::stats::{self, StatsCommand};
use crate::subcommands::{status, stop};

/// Multi-source terminal music player. Omit the subcommand to enter the TUI.
#[derive(Debug, Parser)]
#[command(
    name = "mineral",
    version,
    about = "Mineral — multi-source terminal music player",
    long_about = None,
)]
pub struct Args {
    /// subcommand; omit to launch the TUI
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// 顶层子命令。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// cache management
    Cache {
        /// cache subcommand
        #[command(subcommand)]
        cmd: CacheCommand,
    },

    /// manage music sources
    Channel(ChannelArgs),

    /// user configuration
    Config {
        /// config subcommand
        #[command(subcommand)]
        cmd: ConfigCommand,
    },

    /// control the running daemon (transport, play mode, queue)
    Ctl(CtlArgs),

    /// start the background playback daemon
    Serve,

    /// query analytics data
    Stats {
        /// stats subcommand
        #[command(subcommand)]
        cmd: StatsCommand,
    },

    /// show current playback status
    Status,

    /// exit daemon
    Stop,
}

/// 执行解析后的 CLI 命令。**不处理 [`Command::Serve`]**——那个需要 channels,
/// 由 caller(main.rs) 拦截后自己调 [`crate::serve_run`]。
///
/// # Params:
///   - `command`: 已经从命令行解析出的顶层命令。
///
/// # Return:
///   进程退出码;子命令本身失败(如配置解析)冒泡为 `Err`。
pub fn run(command: Command) -> Result<ExitCode> {
    let runtime = Runtime::new().map_err(Error::Runtime)?;
    runtime.block_on(async move { run_async(command).await })
}

/// 在 tokio 上下文里按 [`Command`] 分发;`Serve` 由 caller(binary)拦截不该到这里。
///
/// 只有 `ctl` 有自己的退出码契约(成功 0 / 被拒 1 / 没执行 3),其余子命令沿用
/// 「成功即 0」。
async fn run_async(command: Command) -> Result<ExitCode> {
    match command {
        Command::Ctl(args) => ctl::run(args).await,
        other => {
            run_plain(other).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// 分发没有自定义退出码契约的子命令。
async fn run_plain(command: Command) -> Result<()> {
    match command {
        Command::Cache { cmd } => cache::run(cmd).await,
        Command::Channel(args) => channel::run(args).await,
        Command::Config { cmd } => config::run(cmd).await,
        Command::Stats { cmd } => stats::run(cmd).await,
        Command::Status => status::run().await,
        Command::Stop => stop::run().await,
        Command::Ctl(_) | Command::Serve => Err(Error::InvalidDispatch),
    }
}

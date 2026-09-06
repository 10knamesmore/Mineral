//! 顶层 CLI 类型与运行入口。

use clap::{Parser, Subcommand};
use color_eyre::eyre::{WrapErr, bail};
use tokio::runtime::Runtime;

use crate::subcommands::action;
use crate::subcommands::cache::{self, CacheCommand};
use crate::subcommands::channel::{self, ChannelArgs};
use crate::subcommands::config::{self, ConfigCommand};
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

    /// connect to a running daemon
    #[arg(long, conflicts_with = "in_proc")]
    pub connect: bool,

    /// run the server inside the TUI process (no daemon / socket)
    #[arg(long)]
    pub in_proc: bool,
}

/// 顶层子命令。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// run a named action registered in `mineral.action`
    Action {
        /// registered action name, e.g. "my.skip_short"
        name: String,

        /// positional arguments passed to the action callback (read via `ctx.args` in Lua); optional
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

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
///   命令执行结果。
pub fn run(command: Command) -> color_eyre::Result<()> {
    let runtime = Runtime::new().wrap_err("create tokio runtime failed")?;
    runtime.block_on(async move { run_async(command).await })?;
    Ok(())
}

/// 在 tokio 上下文里按 [`Command`] 分发到具体子命令;`Serve` 由 caller(binary)拦截不该到这里。
async fn run_async(command: Command) -> color_eyre::Result<()> {
    match command {
        Command::Action { name, args } => action::run(&name, &args).await,
        Command::Cache { cmd } => cache::run(cmd).await,
        Command::Channel(args) => channel::run(args).await,
        Command::Config { cmd } => config::run(cmd).await,
        Command::Stats { cmd } => stats::run(cmd).await,
        Command::Status => status::run().await,
        Command::Stop => stop::run().await,
        Command::Serve => bail!("internal error: Command::Serve must be intercepted by caller"),
    }
}

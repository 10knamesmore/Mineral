//! 顶层 CLI 类型与运行入口。
//!
//! 把命令行参数解析成 [`Args`]，再按顶层 namespace 分发到 [`crate::subcommands`]。
//! 所有具体业务都不在这里——这里只持有 tokio runtime 与 dispatch 表。

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::runtime::Runtime;

use crate::subcommands::channel::{self, ChannelArgs};

/// `mineral` 二进制的顶层参数。
#[derive(Debug, Parser)]
#[command(name = "mineral")]
pub struct Args {
    /// 可选的 CLI 命令；省略时由调用方启动 TUI。
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// 顶层命令分组（按 namespace 拆分）。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// channel 维度的命令，按 channel 名再次分发。
    Channel(ChannelArgs),
}

/// 执行解析后的 CLI 命令。
///
/// # Params:
///   - `command`: 已经从命令行解析出的顶层命令。
///
/// # Return:
///   命令执行结果。
pub fn run(command: Command) -> Result<()> {
    let runtime = Runtime::new().context("create tokio runtime failed")?;
    runtime.block_on(async move { run_async(command).await })?;
    Ok(())
}

async fn run_async(command: Command) -> Result<()> {
    match command {
        Command::Channel(args) => channel::run(args).await,
    }
}

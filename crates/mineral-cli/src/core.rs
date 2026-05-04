//! 顶层 CLI 类型与运行入口。

use clap::{Parser, Subcommand};
use color_eyre::eyre::{WrapErr, bail};
use tokio::runtime::Runtime;

use crate::subcommands::channel::{self, ChannelArgs};
use crate::subcommands::status;

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

    /// 起后台 daemon(单 client 限制),bind unix socket 接受 client 连接。
    /// 实现需要 channels,由 main.rs 拦截后调 [`crate::serve_run`]。
    Serve,

    /// 连 daemon 拉一次 audio 状态打印出来,用于验证 IPC 链路是否通。
    Status,
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

async fn run_async(command: Command) -> color_eyre::Result<()> {
    match command {
        Command::Channel(args) => channel::run(args).await,
        Command::Status => status::run().await,
        Command::Serve => bail!("internal error: Command::Serve must be intercepted by caller"),
    }
}

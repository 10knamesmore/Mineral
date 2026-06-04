//! 顶层 CLI 类型与运行入口。

use clap::{Parser, Subcommand};
use color_eyre::eyre::{WrapErr, bail};
use tokio::runtime::Runtime;

use crate::subcommands::cache::{self, CacheCommand};
use crate::subcommands::channel::{self, ChannelArgs};
use crate::subcommands::config::{self, ConfigCommand};
use crate::subcommands::status;

/// 多源终端音乐播放器。无子命令时进入 TUI。
#[derive(Debug, Parser)]
#[command(
    name = "mineral",
    version,
    about = "Mineral — 多源终端音乐播放器",
    long_about = None,
)]
pub struct Args {
    /// 子命令;省略则进入 TUI。
    #[command(subcommand)]
    pub command: Option<Command>,

    /// 强制连接到已经在跑的后台 daemon(由 `mineral serve` 起),连不上即报错;
    /// 关闭 TUI 时不停 daemon,音乐继续播。默认(不带此 flag)会在没有 daemon 时
    /// 自动 spawn 一个。
    #[arg(long, conflicts_with = "in_proc")]
    pub connect: bool,

    /// in-proc 模式:TUI 自己在同进程内起 server,不走 daemon / socket。
    /// 调试与离线开发用;关闭 TUI = 进程退 = server 一起退。
    #[arg(long)]
    pub in_proc: bool,
}

/// 顶层子命令。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 缓存管理(清理可重建缓存)。
    Cache {
        /// cache 下的具体子命令。
        #[command(subcommand)]
        cmd: CacheCommand,
    },

    /// 管理音乐源(登录、调试)。
    Channel(ChannelArgs),

    /// 用户配置(生成模板 / 校验)。
    Config {
        /// config 下的具体子命令。
        #[command(subcommand)]
        cmd: ConfigCommand,
    },

    /// 启动后台播放 daemon。退出 TUI 后音乐继续播,再开 TUI 用 `--connect` 接回。
    Serve,

    /// 显示当前播放状态(连 daemon)。
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

/// 在 tokio 上下文里按 [`Command`] 分发到具体子命令;`Serve` 由 caller(binary)拦截不该到这里。
async fn run_async(command: Command) -> color_eyre::Result<()> {
    match command {
        Command::Cache { cmd } => cache::run(cmd).await,
        Command::Channel(args) => channel::run(args).await,
        Command::Config { cmd } => config::run(cmd).await,
        Command::Status => status::run().await,
        Command::Serve => bail!("internal error: Command::Serve must be intercepted by caller"),
    }
}

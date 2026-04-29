//! `mineral channel ...` 子命令分发。

use anyhow::Result;
use clap::{Args as ClapArgs, Subcommand};
use mineral_channel_netease::cli::NeteaseCli;

/// `channel` 子命令的参数。
#[derive(Debug, ClapArgs)]
pub struct ChannelArgs {
    /// 选择具体 channel。
    #[command(subcommand)]
    pub channel: ChannelCommand,
}

/// 各个 channel 自己暴露的 CLI 入口。
#[derive(Debug, Subcommand)]
pub enum ChannelCommand {
    /// 网易云 channel。
    Netease(NeteaseCli),
}

/// 执行 `mineral channel ...` 下的命令。
///
/// # Params:
///   - `args`: 已解析的 channel namespace 参数。
///
/// # Return:
///   命令执行结果。
pub async fn run(args: ChannelArgs) -> Result<()> {
    match args.channel {
        ChannelCommand::Netease(cli) => mineral_channel_netease::cli::run(cli).await,
    }
}

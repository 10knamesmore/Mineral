//! `mineral channel ...` 子命令分发。

use clap::{Args as ClapArgs, Subcommand};
use mineral_channel_netease::NeteaseConfig;
use mineral_channel_netease::cli::NeteaseCli;

/// `channel` 子命令的参数。
#[derive(Debug, ClapArgs)]
pub struct ChannelArgs {
    /// 选择音乐源。
    #[command(subcommand)]
    pub channel: ChannelCommand,
}

/// 支持的音乐源。
#[derive(Debug, Subcommand)]
pub enum ChannelCommand {
    /// 网易云音乐(扫码登录、调试 API)。
    Netease(NeteaseCli),
}

/// 执行 `mineral channel ...` 下的命令。
///
/// # Params:
///   - `args`: 已解析的 channel namespace 参数。
///
/// # Return:
///   命令执行结果。
pub async fn run(args: ChannelArgs) -> color_eyre::Result<()> {
    match args.channel {
        ChannelCommand::Netease(cli) => {
            // 自 eval 配置取网易云网络参数:代理 / 超时对扫码登录同样生效。
            let (config, _warnings) =
                mineral_config::load(&mineral_paths::config_dir()?.join("config.lua"))?;
            let nc = netease_config_from(config.sources().netease());
            mineral_channel_netease::cli::run(cli, &nc).await
        }
    }
}

/// 把配置的网易云段映射成构造参数(`NeteaseConfig` 是叶子类型,不依赖配置 crate,
/// 在消费侧做一次显式映射;`mineral` 启动链同样走本函数)。
///
/// # Params:
///   - `section`: 配置的 `sources.netease` 段
///
/// # Return:
///   网易云构造参数。
pub fn netease_config_from(section: &mineral_config::NeteaseSection) -> NeteaseConfig {
    NeteaseConfig::builder()
        .max_connections(*section.max_connections())
        .proxy(section.proxy().clone())
        .timeout_secs(*section.timeout_secs())
        .build()
}

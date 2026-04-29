//! `mineral` 二进制入口。

use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::WrapErr;
use mineral_channel_core::MusicChannel;
use mineral_channel_netease::{load_stored, NeteaseChannel, NeteaseConfig};
use mineral_cli::Args;
use tokio::runtime::Runtime;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let args = Args::parse();
    if let Some(command) = args.command {
        return mineral_cli::run(command);
    }

    let runtime = Runtime::new().wrap_err("create tokio runtime failed")?;
    runtime.block_on(run_tui())
}

async fn run_tui() -> color_eyre::Result<()> {
    let channels = build_channels()?;
    mineral_tui::run(channels).await
}

fn build_channels() -> color_eyre::Result<Vec<Arc<dyn MusicChannel>>> {
    let mut channels = Vec::<Arc<dyn MusicChannel>>::new();
    if let Some(c) = build_netease()? {
        channels.push(c);
    }
    #[cfg(feature = "mock")]
    channels.push(build_mock());
    Ok(channels)
}

fn build_netease() -> color_eyre::Result<Option<Arc<dyn MusicChannel>>> {
    let Some(auth) = load_stored().wrap_err("读取网易云凭证失败")? else {
        return Ok(None);
    };
    let channel =
        NeteaseChannel::with_credential(&NeteaseConfig::default(), &auth.music_u, auth.user_id)
            .wrap_err("构造 NeteaseChannel 失败")?;
    let arc: Arc<dyn MusicChannel> = Arc::new(channel);
    Ok(Some(arc))
}

#[cfg(feature = "mock")]
fn build_mock() -> Arc<dyn MusicChannel> {
    Arc::new(mineral_channel_mock::MockChannel::new())
}

//! `mineral` 二进制入口。

use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::WrapErr;
use mineral_channel_core::MusicChannel;
use mineral_channel_netease::{NeteaseChannel, NeteaseConfig, load_stored};
use mineral_cli::{Args, Command};
use tokio::runtime::Runtime;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // _log_guard 必须持到 main 返回:drop 它会停后台 flush 线程,后续日志丢失。
    let _log_guard = mineral_log::init().wrap_err("init log")?;

    let args = Args::parse();
    match args.command {
        Some(Command::Serve) => {
            // serve 需要 channels(daemon 持有的业务依赖),由 main 自己 build。
            let channels = build_channels()?;
            let runtime = Runtime::new().wrap_err("create tokio runtime failed")?;
            runtime.block_on(mineral_cli::serve_run(channels))
        }
        Some(command) => mineral_cli::run(command),
        None => {
            let runtime = Runtime::new().wrap_err("create tokio runtime failed")?;
            runtime.block_on(run_tui())
        }
    }
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

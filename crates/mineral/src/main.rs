//! `mineral` 二进制入口。

use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::WrapErr;
use mineral_channel_core::MusicChannel;
use mineral_channel_netease::{NeteaseChannel, NeteaseConfig, load_stored};
use mineral_cli::{Args, Command};
use mineral_tui::Launch;
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
            runtime.block_on(run_tui(args.connect, args.in_proc))
        }
    }
}

/// 起 TUI:in-proc 模式自己 build channels;Auto / Connect 跳过(daemon 进程自己持有)。
///
/// `connect` 与 `in_proc` 由 clap `conflicts_with` 保证互斥,故三态映射安全。
async fn run_tui(connect: bool, in_proc: bool) -> color_eyre::Result<()> {
    let launch = if in_proc {
        Launch::InProc
    } else if connect {
        Launch::Connect
    } else {
        Launch::Auto
    };
    // 只有 in-proc 模式 client 与 server 同进程,需要本地 channels;Auto / Connect 下
    // channels 由独立 daemon 进程持有,省去 build_channels 也省去重复读凭证。
    let channels = match launch {
        Launch::InProc => build_channels()?,
        Launch::Auto | Launch::Connect => Vec::new(),
    };
    mineral_tui::run(channels, launch).await
}

/// 按可用凭证 / 编译 feature 收集所有 channel(目前是 netease + 可选 mock)。
fn build_channels() -> color_eyre::Result<Vec<Arc<dyn MusicChannel>>> {
    let mut channels = Vec::<Arc<dyn MusicChannel>>::new();
    if let Some(c) = build_netease()? {
        channels.push(c);
    }
    #[cfg(feature = "mock")]
    channels.push(build_mock());
    Ok(channels)
}

/// 读本地凭证 → 构造 [`NeteaseChannel`];没凭证返回 `Ok(None)`(尚未登录,正常)。
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

/// 构造一个永远在线的假数据 channel,离线开发用(`--features mock`)。
#[cfg(feature = "mock")]
fn build_mock() -> Arc<dyn MusicChannel> {
    Arc::new(mineral_channel_mock::MockChannel::new())
}

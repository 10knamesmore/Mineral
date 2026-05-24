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
        Some(Command::Serve) => run_daemon(),
        Some(command) => mineral_cli::run(command),
        None => {
            let runtime = Runtime::new().wrap_err("create tokio runtime failed")?;
            runtime.block_on(run_tui(args.connect, args.in_proc))
        }
    }
}

/// daemon 入口(`mineral serve`):build channels → 起 runtime → serve。
///
/// daemon 通常被 TUI 以 stderr 重定向的子进程方式拉起,返回的 `Err` 只会进 color-eyre
/// 的 stderr;这里在边界处额外把它写进 **tracing 日志文件**,这样即便 stderr 不可见,
/// 启动失败(如凭证解析失败)也能在日志里查到。
fn run_daemon() -> color_eyre::Result<()> {
    let result = build_channels().and_then(|channels| {
        let runtime = Runtime::new().wrap_err("create tokio runtime failed")?;
        runtime.block_on(mineral_cli::serve_run(channels))
    });
    if let Err(e) = &result {
        mineral_log::error!(target: "daemon", error = mineral_log::chain(e), "daemon 启动失败");
    }
    result
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
///
/// **单个 channel 失败不阻塞**:某源构建失败(如凭证损坏)只 warn + 跳过,不拖垮其他源
/// 或 daemon;空 channels 也是合法状态(没登录任何源),由 TUI 空状态提示兜。
fn build_channels() -> color_eyre::Result<Vec<Arc<dyn MusicChannel>>> {
    let mut channels = Vec::<Arc<dyn MusicChannel>>::new();
    match build_netease() {
        Ok(Some(c)) => channels.push(c),
        Ok(None) => mineral_log::info!(target: "channel", "netease 未登录,跳过"),
        Err(e) => mineral_log::warn!(
            target: "channel",
            error = mineral_log::chain(&e),
            "netease channel 构建失败,跳过(不影响其他源 / daemon)"
        ),
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

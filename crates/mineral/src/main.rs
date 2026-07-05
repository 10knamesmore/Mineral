//! `mineral` 二进制入口。

use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::WrapErr;
use mineral_channel_bilibili::BilibiliChannel;
use mineral_channel_core::MusicChannel;
use mineral_channel_netease::{NeteaseChannel, load_stored};
use mineral_cli::{Args, Command};
use mineral_tui::Launch;
use tokio::runtime::Runtime;

mod os;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // _log_guard 必须持到 main 返回:drop 它会停后台 flush 线程,后续日志丢失。
    let _log_guard = mineral_log::init().wrap_err("init log")?;

    let args = Args::parse();
    match args.command {
        Some(Command::Serve) => os::run_daemon(),
        Some(command) => mineral_cli::run(command),
        None => {
            let runtime = named_runtime("mineral-rt")?;
            runtime.block_on(run_tui(args.connect, args.in_proc))
        }
    }
}

/// 建一个具名的多线程 tokio runtime —— 行为等价默认 `Runtime::new()`(worker 数 =
/// CPU 核数、enable_all),只是给线程起名,便于 `top -H` / perf 里把 mineral 的 tokio
/// 线程跟 isahc-agent、`mineral-audio-rt` 等区分开。
///
/// # Params:
///   - `name`: runtime 线程名(async worker 与 blocking 池线程共用此名 —— tokio 的
///     builder 不单独区分二者)
///
/// # Return:
///   构造好的 runtime;底层 builder 失败时冒泡。
fn named_runtime(name: &'static str) -> color_eyre::Result<Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name(name)
        .build()
        .wrap_err("create tokio runtime failed")
}

/// 在 tokio runtime 上跑完整个 daemon 生命周期(build channels → serve → 优雅收尾)。
///
/// 平台无关的 daemon 主体;主线程归属(直接 block_on,还是让给系统 UI 后台跑)由
/// [`os::run_daemon`] 按平台决定。
///
/// daemon 通常被 TUI 以 stderr 重定向的子进程方式拉起,返回的 `Err` 只会进 color-eyre
/// 的 stderr;这里在边界处额外把它写进 **tracing 日志文件**,这样即便 stderr 不可见,
/// 启动失败(如凭证解析失败)也能在日志里查到。
pub(crate) fn serve_blocking() -> color_eyre::Result<()> {
    let runtime = named_runtime("mineral-daemon-rt")?;
    let result = runtime.block_on(async {
        // daemon 走活 host API:config.lua 顶层的 mineral.* 真实注册,
        // eval 成功的 VM 随 ScriptParts 移交脚本线程(失败已降级纯默认 + 无脚本)。
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (push_tx, push_rx) = tokio::sync::mpsc::unbounded_channel();
        let host = mineral_script::ScriptHost::new(cmd_tx.clone(), push_tx.clone());
        let dir = mineral_paths::config_dir().wrap_err("解析配置目录失败")?;
        let config_path = dir.join("config.lua");
        let (config, warnings, vm) = mineral_config::load_with_vm(&config_path, |lua| {
            mineral_script::install_api(lua, &host).map_err(color_eyre::Report::new)
        })
        .wrap_err("加载用户配置失败")?;
        log_config_warnings(&warnings);
        let script = mineral_server::ScriptParts::new(vm, host, cmd_tx, cmd_rx, push_tx, push_rx);
        let persist = open_persist().await;
        let channels = build_channels(persist.clone(), config.sources())?;
        mineral_cli::serve_run(channels, persist, config, script, config_path).await
    });
    if let Err(e) = &result {
        mineral_log::error!(target: "daemon", error = mineral_log::chain(e), "daemon 启动失败");
    }
    result
}

/// 打开持久化数据库;失败降级为 disabled(warn,不阻断 daemon)。
///
/// # Return:
///   成功返回启用的句柄,失败或路径解析错误返回 disabled 句柄。
async fn open_persist() -> mineral_persist::ServerStore {
    match mineral_paths::data_dir() {
        Ok(dir) => {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                mineral_log::warn!(
                    target: "daemon",
                    error = mineral_log::chain(&e),
                    "建数据目录失败,持久化降级"
                );
                return mineral_persist::ServerStore::disabled();
            }
            match mineral_persist::ServerStore::open(&dir.join("mineral.db")).await {
                Ok(p) => p,
                Err(e) => {
                    mineral_log::warn!(
                        target: "daemon",
                        error = mineral_log::chain(&e),
                        "打开持久化数据库失败,降级 disabled"
                    );
                    mineral_persist::ServerStore::disabled()
                }
            }
        }
        Err(e) => {
            mineral_log::warn!(
                target: "daemon",
                error = mineral_log::chain(&e),
                "定位数据目录失败,持久化降级"
            );
            mineral_persist::ServerStore::disabled()
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
    // in-proc 模式下持久化降级为 disabled:调试路径无需落盘。
    let (config, warnings) = load_config()?;
    log_config_warnings(&warnings);
    let (channels, persist) = match launch {
        Launch::InProc => {
            let p = mineral_persist::ServerStore::disabled();
            let ch = build_channels(p.clone(), config.sources())?;
            (ch, p)
        }
        Launch::Auto | Launch::Connect => (Vec::new(), mineral_persist::ServerStore::disabled()),
    };
    mineral_tui::run(channels, launch, persist, config, warnings).await
}

/// 加载用户配置:config 目录解析失败或内置 default.lua 损坏(程序员错误)时冒泡;
/// 用户 `config.lua` 的错误已在 loader 内降级为 warnings,不会让加载失败。
fn load_config() -> color_eyre::Result<(mineral_config::Config, Vec<mineral_config::ConfigWarning>)>
{
    let dir = mineral_paths::config_dir().wrap_err("解析配置目录失败")?;
    mineral_config::load(&dir.join("config.lua")).wrap_err("加载用户配置失败")
}

/// 把配置降级告警逐条落日志(daemon 无 UI,日志是唯一出口;TUI 另有 toast)。
fn log_config_warnings(warnings: &[mineral_config::ConfigWarning]) {
    for w in warnings {
        mineral_log::warn!(target: "config", warning = %w, "用户配置降级");
    }
}

/// 按可用凭证 / 编译 feature 收集所有 channel(目前是 netease + 可选 mock)。
///
/// **单个 channel 失败不阻塞**:某源构建失败(如凭证损坏)只 warn + 跳过,不拖垮其他源
/// 或 daemon;空 channels 也是合法状态(没登录任何源),由 TUI 空状态提示兜。
///
/// # Params:
///   - `persist`: 持久化句柄,注入各 channel 供登录状态/统计落盘使用。
///   - `sources`: 音乐源段配置(netease 的 timeout / proxy / 并发)。
fn build_channels(
    persist: mineral_persist::ServerStore,
    sources: &mineral_config::SourcesConfig,
) -> color_eyre::Result<Vec<Arc<dyn MusicChannel>>> {
    let mut channels = Vec::<Arc<dyn MusicChannel>>::new();
    match build_netease(persist, sources.netease()) {
        Ok(Some(c)) => channels.push(c),
        Ok(None) => mineral_log::info!(target: "channel", "netease 未登录,跳过"),
        Err(e) => mineral_log::warn!(
            target: "channel",
            error = mineral_log::chain(&e),
            "netease channel 构建失败,跳过(不影响其他源 / daemon)"
        ),
    }
    // B站 guest 模式无需登录即可搜索/详情/取流,故恒尝试构建(登录 stage 3 再加)。
    match build_bilibili(sources.bilibili()) {
        Ok(c) => channels.push(c),
        Err(e) => mineral_log::warn!(
            target: "channel",
            error = mineral_log::chain(&e),
            "bilibili channel 构建失败,跳过(不影响其他源 / daemon)"
        ),
    }
    #[cfg(feature = "mock")]
    channels.push(build_mock());
    Ok(channels)
}

/// 构造 guest B站 channel(公开端点:搜索 / 详情 / 取流,无需登录)。
///
/// 与 netease 不同,B站不需要预存凭证即可用,故恒返回一个 channel(非 `Option`);
/// 登录(解锁高码率 / 私密收藏夹)是后续阶段。
///
/// # Params:
///   - `bilibili`: B站源段配置(timeout / proxy / 并发)。
fn build_bilibili(
    bilibili: &mineral_config::BilibiliSection,
) -> color_eyre::Result<Arc<dyn MusicChannel>> {
    let bc = mineral_cli::bilibili_config_from(bilibili);
    // 有存储凭证 → 带登录态(解锁我的收藏夹 / 高码率);否则 guest。
    let channel = match mineral_channel_bilibili::load_stored().wrap_err("读取 B站凭证失败")?
    {
        Some(auth) => BilibiliChannel::with_credential(&bc, &auth)
            .wrap_err("构造带登录态 BilibiliChannel 失败")?,
        None => BilibiliChannel::new(&bc).wrap_err("构造 BilibiliChannel 失败")?,
    };
    let arc: Arc<dyn MusicChannel> = Arc::new(channel);
    Ok(arc)
}

/// 读本地凭证 → 构造 [`NeteaseChannel`];没凭证返回 `Ok(None)`(尚未登录,正常)。
/// 早返回在构造 `NeteaseConfig` 之前 —— config 注入不改未登录降级路径。
///
/// # Params:
///   - `persist`: 持久化句柄,传入 channel 供登录状态/统计落盘使用。
///   - `netease`: 网易云源段配置(timeout / proxy / 并发)。
fn build_netease(
    persist: mineral_persist::ServerStore,
    netease: &mineral_config::NeteaseSection,
) -> color_eyre::Result<Option<Arc<dyn MusicChannel>>> {
    let Some(auth) = load_stored().wrap_err("读取网易云凭证失败")? else {
        return Ok(None);
    };
    let nc = mineral_cli::netease_config_from(netease);
    let channel = NeteaseChannel::with_credential(&nc, &auth.music_u, auth.user_id, persist)
        .wrap_err("构造 NeteaseChannel 失败")?;
    let arc: Arc<dyn MusicChannel> = Arc::new(channel);
    Ok(Some(arc))
}

/// 构造一个永远在线的假数据 channel,离线开发用(`--features mock`)。
#[cfg(feature = "mock")]
fn build_mock() -> Arc<dyn MusicChannel> {
    Arc::new(mineral_channel_mock::MockChannel::new())
}

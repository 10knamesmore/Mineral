//! `mineral` 二进制入口。

use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::WrapErr;
use mineral_channel_bilibili::BilibiliChannel;
use mineral_channel_core::MusicChannel;
use mineral_channel_netease::{NeteaseChannel, load_stored};
use mineral_cli::{Args, Command};
use mineral_config::DaemonLoad;
use mineral_playback::{PlaybackProvider, PlaybackRegistry};
use mineral_tui::Launch;
use tokio::runtime::Runtime;

mod os;

/// 全局分配器换成 dhat,供 `dhat::Profiler` 记录每次分配的调用栈 + 字节(仅 `dhat-heap`
/// feature 构建时生效);无 Profiler 时透传,故 daemon 进程不受影响。
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // _log_guard 必须持到 main 返回:drop 它会停后台 flush 线程,后续日志丢失。
    let _log_guard = mineral_log::init().wrap_err("init log")?;

    let args = Args::parse();
    match args.command {
        Some(Command::Serve) => os::run_daemon(),
        Some(command) => mineral_cli::run(command),
        None => {
            // dhat guard 必须持到 TUI 退出:Drop 时才落 dhat-heap.json。
            #[cfg(feature = "dhat-heap")]
            let _dhat = dhat::Profiler::new_heap();
            let runtime = named_runtime("mineral-rt")?;
            runtime.block_on(run_tui(args.connect))
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
        let loaded = mineral_config::load_with_vm(&config_path, |lua| {
            mineral_script::install_api(lua, &host).map_err(color_eyre::Report::new)
        })
        .wrap_err("加载用户配置失败")?;
        log_config_warnings(&loaded.warnings);

        let DaemonLoad {
            config,
            vm,
            tree: config_tree,
            ..
        } = loaded;
        let script = mineral_server::ScriptParts::new(vm, host, cmd_tx, cmd_rx, push_tx, push_rx);
        let persist = open_persist().await;
        let sources = build_sources(persist.clone(), config.sources())?;
        mineral_cli::serve_run(
            sources.channels,
            sources.playback,
            persist,
            config,
            script,
            config_tree,
            config_path,
        )
        .await
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

/// 起 TUI:Auto 优先 attach 已有 daemon、没有则 spawn 一个独立 daemon 再 attach;
/// Connect 强制连已有 daemon。
async fn run_tui(connect: bool) -> color_eyre::Result<()> {
    let launch = if connect {
        Launch::Connect
    } else {
        Launch::Auto
    };
    let (config, warnings) = load_config()?;
    log_config_warnings(&warnings);
    mineral_tui::run(launch, config, warnings).await
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

/// 构造内建 channel 与对应的 playback provider。
///
/// Mineral 聚合 channel 恒注册。单个远端源构建失败(如凭证损坏)只 warn + 跳过,
/// 不阻塞其他源或 daemon。
///
/// # Params:
///   - `persist`: 持久化句柄,注入各 channel 供登录状态/统计落盘使用。
///   - `sources`: 音乐源段配置(netease 的 timeout / proxy / 并发)。
fn build_sources(
    persist: mineral_persist::ServerStore,
    sources: &mineral_config::SourcesConfig,
) -> color_eyre::Result<BuiltSources> {
    let mut channels = Vec::<Arc<dyn MusicChannel>>::new();
    let mut providers = Vec::<Arc<dyn PlaybackProvider>>::new();
    // 聚合源(全源收藏投影):纯 persist 投影、无凭证依赖,恒注册。放列表首位,
    // 其歌单列表(本地 SQL)最先就绪,聚合收藏歌单自然排 sidebar 顶部。
    channels.push(Arc::new(mineral_channel_mineral::MineralChannel::new(
        persist.clone(),
    )));
    match build_netease(persist, sources.netease()) {
        Ok(Some(pair)) => {
            channels.push(pair.channel);
            providers.push(pair.provider);
        }
        Ok(None) => mineral_log::info!(target: "channel", "netease 未登录,跳过"),
        Err(e) => mineral_log::warn!(
            target: "channel",
            error = mineral_log::chain(&e),
            "netease channel 构建失败,跳过(不影响其他源 / daemon)"
        ),
    }
    // B站 guest 模式无需登录即可搜索/详情/取流,故恒尝试构建。
    match build_bilibili(sources.bilibili()) {
        Ok(pair) => {
            channels.push(pair.channel);
            providers.push(pair.provider);
        }
        Err(e) => mineral_log::warn!(
            target: "channel",
            error = mineral_log::chain(&e),
            "bilibili channel 构建失败,跳过(不影响其他源 / daemon)"
        ),
    }
    Ok(BuiltSources {
        channels,
        playback: PlaybackRegistry::new(providers)?,
    })
}

/// 构造 B站 channel；有已存凭证时带登录态，否则使用 guest 模式。
///
/// guest 模式可访问搜索、详情与取流等公开端点；登录态额外提供我的收藏夹与高码率。
///
/// # Params:
///   - `bilibili`: B站源段配置(timeout / proxy / 并发)。
fn build_bilibili(bilibili: &mineral_config::BilibiliSection) -> color_eyre::Result<SourcePair> {
    let bc = mineral_cli::bilibili_config_from(bilibili);
    // 有存储凭证 → 带登录态(解锁我的收藏夹 / 高码率);否则 guest。
    let channel = match mineral_channel_bilibili::load_stored().wrap_err("读取 B站凭证失败")?
    {
        Some(auth) => BilibiliChannel::with_credential(&bc, &auth)
            .wrap_err("构造带登录态 BilibiliChannel 失败")?,
        None => BilibiliChannel::new(&bc).wrap_err("构造 BilibiliChannel 失败")?,
    };
    let concrete = Arc::new(channel);
    let channel: Arc<dyn MusicChannel> = concrete.clone();
    let provider: Arc<dyn PlaybackProvider> = concrete;
    Ok(SourcePair { channel, provider })
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
) -> color_eyre::Result<Option<SourcePair>> {
    let Some(auth) = load_stored().wrap_err("读取网易云凭证失败")? else {
        return Ok(None);
    };
    let nc = mineral_cli::netease_config_from(netease);
    let channel = NeteaseChannel::with_credential(&nc, &auth.music_u, auth.user_id, persist)
        .wrap_err("构造 NeteaseChannel 失败")?;
    let concrete = Arc::new(channel);
    let channel: Arc<dyn MusicChannel> = concrete.clone();
    let provider: Arc<dyn PlaybackProvider> = concrete;
    Ok(Some(SourcePair { channel, provider }))
}

/// Channel and playback dependencies assembled for one process.
struct BuiltSources {
    /// Catalog, library, and user-data connectors.
    channels: Vec<Arc<dyn MusicChannel>>,

    /// Playback resource providers keyed by source identity.
    playback: PlaybackRegistry,
}

/// Sibling channel and playback provider handles for one concrete source adapter.
struct SourcePair {
    /// Catalog, library, and user-data connector.
    channel: Arc<dyn MusicChannel>,

    /// Playback resource provider.
    provider: Arc<dyn PlaybackProvider>,
}

//! Terminal UI client for Mineral.

#[cfg(windows)]
compile_error!("Windows 暂不支持");

mod app;
mod components;
mod image;
mod player_actions;
mod render;
mod runtime;
#[cfg(test)]
mod test_support;
mod tui;
mod view;

use std::sync::Arc;

use mineral_server::Client;

use app::App;
use image::ImageEngine;
use image::fetch::CoverFetcher;
use image::graphics::TerminalGraphics;
use runtime::ui::prefs::{UiPrefs, open_client_store};
use tui::Tui;

/// 启动 TUI:优先 attach 已有 daemon、没有则 spawn 一个独立 `mineral serve`
/// 子进程再 attach。
///
/// # Params:
///   - `config`: 已加载的用户配置(TUI bootstrap 用,连上 daemon 后由推送顶替)。
///   - `warnings`: 配置降级告警,启动后经通知层 toast 呈现。
pub async fn run(
    config: mineral_config::Config,
    warnings: Vec<mineral_config::ConfigWarning>,
) -> color_eyre::Result<()> {
    // 用户配置降级告警:先落日志,启动后再经通知层 toast 呈现(run_app 内)。
    for w in &warnings {
        mineral_log::warn!(target: "config", warning = %w, "用户配置降级");
    }
    let cfg = Arc::new(config);
    // tui.db 一次打开,封面缓存索引与 UI 偏好共用一个连接池;打不开整体降级
    // (封面不缓存、偏好不存不读),其余照常。
    let store = open_client_store().await;
    let ui_prefs = UiPrefs::load(store.clone()).await;
    // 封面 fetcher 起不来(isahc / TLS / 证书)不该拖垮整个 TUI —— 降级到禁用态空跑,
    // 与音频无设备降级 null 模式同理。封面不显示,其余功能照常。
    let cover_fetcher = CoverFetcher::spawn(
        cfg.tui().cover().clone(),
        *cfg.tui().cover().cache().disk(),
        store,
    )
    .await
    .unwrap_or_else(|e| {
        mineral_log::warn!(
            error = mineral_log::chain(&e),
            "cover fetcher 起步失败,封面禁用"
        );
        CoverFetcher::disabled()
    });

    let socket = mineral_paths::socket_path()?;
    let kill_on_exit = *cfg.tui().behavior().kill_spawned_daemon_on_exit();
    let (client, handle) = runtime::daemon::ensure(&socket, kill_on_exit).await?;
    let result = run_app(Arc::new(client), cover_fetcher, ui_prefs, cfg, &warnings);
    // client 退出:仅当本次亲手 spawn 了 daemon 才按旋钮收尾;attach 已有的
    // (handle 为 None)留着不动。
    if let Some(handle) = handle {
        handle.shutdown_if_owned();
    }
    result
}

/// 拿到一个 client(经 IPC 连 daemon),进 alternate screen,探测终端图片能力,
/// 跑 [`App::run`] 直到退出,最后还原终端。
///
/// # Params:
///   - `ui_prefs`: 已读回初值的 UI 偏好句柄(歌词副轨档等,`App::new` 内落地)
///   - `cfg`: 已加载的全局配置(主题 / 键表 / 各段手感在 `App::new` 内落地)
///   - `warnings`: 配置降级告警,启动后经通知层 toast 呈现
fn run_app(
    client: Arc<dyn Client>,
    cover_fetcher: CoverFetcher,
    ui_prefs: UiPrefs,
    cfg: Arc<mineral_config::Config>,
    warnings: &[mineral_config::ConfigWarning],
) -> color_eyre::Result<()> {
    let mut tui = Tui::new()?;
    tui.enter()?;
    // 标题栈 push 与配置开关对称;禁用时不 push,退出也无需 pop。运行时热重载从禁用→
    // 启用由主循环幂等兜底补 push(见 App::run),故此处只管启动即启用的常路。
    if *cfg.tui().window_title().enabled() {
        tui.push_title_stack()?;
    }
    // 图片能力探测必须在进 alternate screen 之后、读 events 之前执行，因为底层会临时
    // 往 stdio 写探测 escape 序列并读取响应。
    let graphics = TerminalGraphics::query();
    let images = ImageEngine::new(Arc::clone(&cfg), cover_fetcher, graphics);
    let mut app = App::new(client, images, tui.launch_cursor(), cfg, ui_prefs);
    // 启动期配置提示卡:config.lua 缺失提醒 init;降级告警与日志双轨呈现。
    let config_path = mineral_paths::config_dir()
        .ok()
        .map(|d| d.join("config.lua"));
    app.notify_startup_config(config_path.as_deref(), warnings);
    let result = app.run(&mut tui);
    tui.exit()?;
    result
}

//! config 子命令调用两个宿主的配置入口;文件工作不阻塞异步执行器。

use std::io::IsTerminal;

use clap::Subcommand;

/// config subcommand
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// generate daemon and TUI config templates
    Init,

    /// load and validate both configs without running setup
    Check,
}

/// 在阻塞任务中生成资产或校验两份配置,不访问第三份共享用户文件。
pub async fn run(command: ConfigCommand) -> crate::error::Result<()> {
    tokio::task::spawn_blocking(move || match command {
        ConfigCommand::Init => init(),
        ConfigCommand::Check => check(),
    })
    .await
    .map_err(crate::error::Error::ConfigTask)?
}

/// 初始化两个宿主资产并逐项打印写入结果。
fn init() -> crate::error::Result<()> {
    let dir = mineral_paths::config_dir()?;
    for outcome in super::init::run_init(&dir)? {
        println!("{outcome}");
    }
    Ok(())
}

/// 各宿主独立落型,渲染摘要;不运行 setup。
fn check() -> crate::error::Result<()> {
    let dir = mineral_paths::config_dir()?;
    let (config, warnings) = mineral_server::config::load_daemon(&dir.join("daemon.lua"))?;
    for warning in &warnings {
        mineral_log::warn!(target: "config", host = "daemon", error = mineral_log::chain(warning), "configuration check fell back to defaults");
    }
    let (tui, tui_warnings) = mineral_tui::config::load_tui(&dir.join("tui.lua"))?;
    for warning in &tui_warnings {
        mineral_log::warn!(target: "config", host = "TUI", error = mineral_log::chain(warning), "configuration check fell back to defaults");
    }
    let default_download_dir = mineral_paths::music_export_dir()?;
    let color = std::io::stdout().is_terminal();
    println!(
        "{}",
        mineral_server::config::render_check(&config, &warnings, &default_download_dir, color)
    );
    println!(
        "{}",
        mineral_tui::config::render_check(&tui, &tui_warnings, color)
    );
    mineral_log::info!(target: "config", daemon_warnings = warnings.len(), tui_warnings = tui_warnings.len(), "daemon and TUI configuration checked without setup");
    Ok(())
}

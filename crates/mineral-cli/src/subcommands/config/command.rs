//! `config` 子命令定义与执行:落地模板 / 渲染诊断,逻辑转调 `mineral-config`。

use std::io::IsTerminal;

use clap::Subcommand;

/// config 下的具体子命令。
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// 生成配置模板 + LSP stub + `.luarc.json`(已存在的 config.lua 不覆盖)。
    Init,

    /// 加载并校验配置,打印诊断(有效配置摘要 + 警告)。
    Check,
}

/// 执行 config 子命令。
///
/// # Params:
///   - `command`: 具体子命令。
///
/// # Return:
///   执行结果。
pub async fn run(command: ConfigCommand) -> color_eyre::Result<()> {
    match command {
        ConfigCommand::Init => init(),
        ConfigCommand::Check => check(),
    }
}

/// 生成配置资产到 config 目录,逐行打印写入 / 跳过结果。
///
/// # Return:
///   执行结果。
fn init() -> color_eyre::Result<()> {
    let dir = mineral_paths::config_dir()?;
    for outcome in mineral_config::run_init(&dir)? {
        println!("{outcome}");
    }
    Ok(())
}

/// 加载并渲染配置诊断(tty 时上色)。
///
/// # Return:
///   执行结果。
fn check() -> color_eyre::Result<()> {
    let dir = mineral_paths::config_dir()?;
    let (config, warnings) = mineral_config::load(&dir.join("config.lua"))?;
    let default_download_dir = mineral_paths::music_export_dir()?;
    let color = std::io::stdout().is_terminal();
    println!(
        "{}",
        mineral_config::render_check(&config, &warnings, &default_download_dir, color)
    );
    Ok(())
}

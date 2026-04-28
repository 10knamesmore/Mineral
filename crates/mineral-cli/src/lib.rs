//! Command-line subcommands for Mineral.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
use isahc::http::Uri;
use mineral_channel_netease::api::login::{login_qr_check, login_qr_get_key};
use mineral_channel_netease::{NeteaseChannel, NeteaseConfig};
use qrcode::render::unicode;
use qrcode::QrCode;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

const NETEASE_BASE_URL: &str = "https://music.163.com";
const LOGIN_STATUS_WAIT_SCAN: i64 = 801;
const LOGIN_STATUS_WAIT_CONFIRM: i64 = 802;
const LOGIN_STATUS_SUCCESS: i64 = 803;
const LOGIN_STATUS_EXPIRED: i64 = 800;

/// Top-level arguments for the `mineral` binary.
#[derive(Debug, Parser)]
#[command(name = "mineral")]
pub struct Args {
    /// Optional CLI command. When omitted, starts the TUI client.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Supported CLI command groups.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Channel-specific commands.
    Channel(ChannelArgs),
}

/// Supported channel namespaces.
#[derive(Debug, ClapArgs)]
pub struct ChannelArgs {
    /// Channel-specific subcommands.
    #[command(subcommand)]
    pub channel: ChannelCommand,
}

/// Channel-specific command tree.
#[derive(Debug, Subcommand)]
pub enum ChannelCommand {
    /// Commands for the Netease channel.
    Netease(NeteaseArgs),
}

/// CLI commands under `channel netease`.
#[derive(Debug, ClapArgs)]
pub struct NeteaseArgs {
    /// The concrete Netease action to run.
    #[command(subcommand)]
    pub command: NeteaseCommand,
}

/// Supported Netease CLI actions.
#[derive(Debug, Subcommand)]
pub enum NeteaseCommand {
    /// Log in to Netease Music by scanning a terminal QR code.
    Login,
}

/// Execute a parsed CLI command.
pub fn run(command: Command) -> Result<()> {
    let runtime = Runtime::new().context("create tokio runtime failed")?;
    runtime.block_on(async move { run_async(command).await })?;
    Ok(())
}

async fn run_async(command: Command) -> Result<()> {
    match command {
        Command::Channel(channel) => match channel.channel {
            ChannelCommand::Netease(netease) => match netease.command {
                NeteaseCommand::Login => run_netease_login().await,
            },
        },
    }
}

async fn run_netease_login() -> Result<()> {
    let channel = NeteaseChannel::new(&NeteaseConfig::default())?;
    let qr = login_qr_get_key(channel.transport()).await?;
    render_qr(&qr.url)?;
    eprintln!("等待网易云 App 扫码并确认...");

    let mut last_status: Option<i64> = None;
    loop {
        let status = login_qr_check(channel.transport(), &qr.unikey).await?;
        if last_status != Some(status) {
            print_status_hint(status);
            last_status = Some(status);
        }

        match status {
            LOGIN_STATUS_WAIT_SCAN | LOGIN_STATUS_WAIT_CONFIRM => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => return Err(anyhow!("二维码登录已取消")),
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
            LOGIN_STATUS_SUCCESS => {
                let music_u = extract_music_u(&channel)?;
                let auth = StoredNeteaseAuth { music_u };
                let path = netease_auth_path()?;
                write_netease_auth_file(&path, &auth)?;
                println!("登录成功，凭证已写入 {}", path.display());
                return Ok(());
            }
            LOGIN_STATUS_EXPIRED => {
                return Err(anyhow!("二维码已过期，请重新执行登录命令"));
            }
            other => {
                return Err(anyhow!("未知二维码登录状态码: {other}"));
            }
        }
    }
}

fn render_qr(url: &str) -> Result<()> {
    let code = QrCode::new(url.as_bytes()).context("生成二维码失败")?;
    let rendered = code.render::<unicode::Dense1x2>().quiet_zone(true).build();
    println!("{rendered}");
    Ok(())
}

fn print_status_hint(status: i64) {
    match status {
        LOGIN_STATUS_WAIT_SCAN => eprintln!("状态: 等待扫码"),
        LOGIN_STATUS_WAIT_CONFIRM => eprintln!("状态: 等待手机确认"),
        LOGIN_STATUS_SUCCESS | LOGIN_STATUS_EXPIRED => {}
        other => eprintln!("状态: {other}"),
    }
}

fn extract_music_u(channel: &NeteaseChannel) -> Result<String> {
    let jar = channel
        .transport()
        .cookie_jar()
        .ok_or_else(|| anyhow!("二维码登录后未拿到 cookie jar"))?;
    let uri: Uri = NETEASE_BASE_URL.parse().context("parse netease base uri failed")?;
    let cookie = jar
        .get_by_name(&uri, "MUSIC_U")
        .ok_or_else(|| anyhow!("二维码登录成功，但未在 cookie jar 中找到 MUSIC_U"))?;
    Ok(cookie.value().to_owned())
}

fn netease_auth_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME 未设置，无法确定凭证路径"))?;
    Ok(PathBuf::from(home).join(".local/mineral/netease.json"))
}

fn write_netease_auth_file(path: &Path, auth: &StoredNeteaseAuth) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("netease 凭证路径缺少父目录"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create credential dir failed: {}", parent.display()))?;
    let json = serde_json::to_string_pretty(auth).context("serialize netease auth failed")?;
    fs::write(path, json)
        .with_context(|| format!("write netease auth failed: {}", path.display()))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredNeteaseAuth {
    music_u: String,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{write_netease_auth_file, StoredNeteaseAuth};

    #[test]
    fn write_netease_auth_file_persists_json() {
        let base = std::env::temp_dir().join(format!("mineral-cli-test-{}", std::process::id()));
        let path = base.join("netease.json");
        let auth = StoredNeteaseAuth {
            music_u: String::from("abc"),
        };

        write_netease_auth_file(&path, &auth).expect("write auth file");

        let raw = std::fs::read_to_string(&path).expect("read auth file");
        assert!(raw.contains("\"music_u\": \"abc\""));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(PathBuf::from(base));
    }
}

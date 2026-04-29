//! 网易云 channel 的 CLI 子命令实现。
//!
//! 顶层 [`mineral-cli`] 通过 [`NeteaseCli`] 把 `mineral channel netease ...` 这一支
//! 整体转发到这里，具体的登录流程、二维码渲染、凭证写入都在本模块内闭环。

use std::time::Duration;

use clap::{Args as ClapArgs, Subcommand};
use color_eyre::eyre::{eyre, WrapErr};
use isahc::http::Uri;
use qrcode::render::unicode;
use qrcode::QrCode;

use crate::api::login::{login_qr_check, login_qr_get_key};
use crate::api::user::account_uid;
use crate::credential::{save, StoredNeteaseAuth};
use crate::{NeteaseChannel, NeteaseConfig};

const NETEASE_BASE_URL: &str = "https://music.163.com";
const LOGIN_STATUS_WAIT_SCAN: i64 = 801;
const LOGIN_STATUS_WAIT_CONFIRM: i64 = 802;
const LOGIN_STATUS_SUCCESS: i64 = 803;
const LOGIN_STATUS_EXPIRED: i64 = 800;

/// 网易云 channel 的 CLI 入口（`mineral channel netease ...`）。
#[derive(Debug, ClapArgs)]
pub struct NeteaseCli {
    /// 网易云下的具体子命令。
    #[command(subcommand)]
    pub command: NeteaseCommand,
}

/// 支持的网易云 CLI 操作。
#[derive(Debug, Subcommand)]
pub enum NeteaseCommand {
    /// 终端二维码登录网易云。
    Login,
}

/// 执行解析后的网易云 CLI 命令。
pub async fn run(cli: NeteaseCli) -> color_eyre::Result<()> {
    match cli.command {
        NeteaseCommand::Login => run_login().await,
    }
}

async fn run_login() -> color_eyre::Result<()> {
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
                    _ = tokio::signal::ctrl_c() => return Err(eyre!("二维码登录已取消")),
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
            LOGIN_STATUS_SUCCESS => {
                let music_u = extract_music_u(&channel)?;
                let user_id = account_uid(channel.transport())
                    .await
                    .context("登录成功但未能拉到 userId")?;
                let auth = StoredNeteaseAuth { music_u, user_id };
                let path = save(&auth)?;
                println!("登录成功，凭证已写入 {}", path.display());
                return Ok(());
            }
            LOGIN_STATUS_EXPIRED => {
                return Err(eyre!("二维码已过期，请重新执行登录命令"));
            }
            other => {
                return Err(eyre!("未知二维码登录状态码: {other}"));
            }
        }
    }
}

fn render_qr(url: &str) -> color_eyre::Result<()> {
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

fn extract_music_u(channel: &NeteaseChannel) -> color_eyre::Result<String> {
    let jar = channel
        .transport()
        .cookie_jar()
        .ok_or_else(|| eyre!("二维码登录后未拿到 cookie jar"))?;
    let uri: Uri = NETEASE_BASE_URL
        .parse()
        .context("parse netease base uri failed")?;
    let cookie = jar
        .get_by_name(&uri, "MUSIC_U")
        .ok_or_else(|| eyre!("二维码登录成功，但未在 cookie jar 中找到 MUSIC_U"))?;
    Ok(cookie.value().to_owned())
}

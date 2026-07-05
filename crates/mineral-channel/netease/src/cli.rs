//! 网易云 channel 的 CLI 子命令实现。
//!
//! 顶层 [`mineral-cli`] 通过 [`NeteaseCli`] 把 `mineral channel netease ...` 这一支
//! 整体转发到这里，具体的登录流程、二维码渲染、凭证写入都在本模块内闭环。

use std::time::Duration;

use clap::{Args as ClapArgs, Subcommand};
use color_eyre::eyre::{WrapErr, eyre};
use isahc::http::Uri;
use qrcode::QrCode;
use qrcode::render::unicode;

use crate::api::login::{login_qr_check, login_qr_get_key};
use crate::api::user::account_uid;
use crate::credential::{StoredNeteaseAuth, save};
use crate::{NeteaseChannel, NeteaseConfig};

/// 网易云的根 URL,用于在 cookie jar 中按域名定位 MUSIC_U。
const NETEASE_BASE_URL: &str = "https://music.163.com";

/// 二维码状态:已生成、等待用户扫码。
const LOGIN_STATUS_WAIT_SCAN: i64 = 801;

/// 二维码状态:已扫码,等待用户在手机上点确认。
const LOGIN_STATUS_WAIT_CONFIRM: i64 = 802;

/// 二维码状态:登录成功,jar 中已写入 MUSIC_U。
const LOGIN_STATUS_SUCCESS: i64 = 803;

/// 二维码状态:已过期,需重新生成。
const LOGIN_STATUS_EXPIRED: i64 = 800;

/// 网易云音乐操作。
#[derive(Debug, ClapArgs)]
pub struct NeteaseCli {
    /// 选择操作。
    #[command(subcommand)]
    pub command: NeteaseCommand,
}

/// 网易云音乐子命令。
#[derive(Debug, Subcommand)]
pub enum NeteaseCommand {
    /// 扫码登录
    Login,
}

/// 执行解析后的网易云 CLI 命令。
///
/// # Params:
///   - `cli`: 已解析的子命令
///   - `config`: 网易云构造参数(由调用方自配置派生;代理/超时对登录同样生效)
pub async fn run(cli: NeteaseCli, config: &NeteaseConfig) -> color_eyre::Result<()> {
    match cli.command {
        NeteaseCommand::Login => run_login(config).await,
    }
}

/// `mineral channel netease login` 的主流程:取 unikey、终端渲染二维码、轮询状态、登录成功后写凭证。
async fn run_login(config: &NeteaseConfig) -> color_eyre::Result<()> {
    let channel = NeteaseChannel::new(config, mineral_persist::ServerStore::disabled())?;
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

/// 把 url 编成二维码并按 unicode dense 1x2 字符块输出到 stdout。
fn render_qr(url: &str) -> color_eyre::Result<()> {
    let code = QrCode::new(url.as_bytes()).context("生成二维码失败")?;
    let rendered = code.render::<unicode::Dense1x2>().quiet_zone(true).build();
    println!("{rendered}");
    Ok(())
}

/// 把轮询状态码翻成中文人话提示,过渡态打到 stderr,终态由调用方处理。
fn print_status_hint(status: i64) {
    match status {
        LOGIN_STATUS_WAIT_SCAN => eprintln!("状态: 等待扫码"),
        LOGIN_STATUS_WAIT_CONFIRM => eprintln!("状态: 等待手机确认"),
        LOGIN_STATUS_SUCCESS | LOGIN_STATUS_EXPIRED => {}
        other => eprintln!("状态: {other}"),
    }
}

/// 从 channel 持有的 cookie jar 里按 `NETEASE_BASE_URL` 取出 `MUSIC_U` 的值。
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

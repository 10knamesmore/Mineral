//! 哔哩哔哩 channel 的 CLI:二维码登录。
//!
//! 顶层 `mineral-cli` 把 `mineral channel bilibili ...` 整支转发到这里;登录流程、二维码渲染、
//! 凭证写入都在本模块闭环。

use std::time::Duration;

use clap::{Args as ClapArgs, Subcommand};
use color_eyre::eyre::{WrapErr, eyre};
use isahc::http::Uri;
use qrcode::QrCode;
use qrcode::render::unicode;

use crate::api::login::{generate, poll};
use crate::credential::{StoredBilibiliAuth, save};
use crate::{BilibiliChannel, BilibiliConfig};

/// B站根 URL,用于在 cookie jar 中按域名定位凭证 cookie。
const BASE_URL: &str = "https://www.bilibili.com";

/// 登录状态:成功(jar 已写入凭证)。
const STATUS_SUCCESS: i64 = 0;

/// 登录状态:二维码已生成,等待扫码。
const STATUS_NOT_SCANNED: i64 = 86101;

/// 登录状态:已扫码,等待手机确认。
const STATUS_NOT_CONFIRMED: i64 = 86090;

/// 登录状态:二维码已失效。
const STATUS_EXPIRED: i64 = 86038;

/// 哔哩哔哩操作。
#[derive(Debug, ClapArgs)]
pub struct BilibiliCli {
    /// 选择操作。
    #[command(subcommand)]
    pub command: BilibiliCommand,
}

/// 哔哩哔哩子命令。
#[derive(Debug, Subcommand)]
pub enum BilibiliCommand {
    /// 扫码登录
    Login,
}

/// 执行解析后的 B站 CLI 命令。
///
/// # Params:
///   - `cli`: 已解析的子命令
///   - `config`: B站构造参数(代理 / 超时对登录同样生效)
pub async fn run(cli: BilibiliCli, config: &BilibiliConfig) -> color_eyre::Result<()> {
    match cli.command {
        BilibiliCommand::Login => run_login(config).await,
    }
}

/// `mineral channel bilibili login` 主流程:申请二维码、终端渲染、轮询、成功后写凭证。
async fn run_login(config: &BilibiliConfig) -> color_eyre::Result<()> {
    let channel = BilibiliChannel::new(config)?;
    let qr = generate(channel.transport()).await?;
    render_qr(&qr.url)?;
    eprintln!("等待 B站 App 扫码并确认...");

    let mut last: Option<i64> = None;
    loop {
        let status = poll(channel.transport(), &qr.qrcode_key).await?;
        if last != Some(status.code) {
            print_status_hint(status.code);
            last = Some(status.code);
        }
        match status.code {
            STATUS_SUCCESS => {
                let auth = extract_auth(&channel)?;
                let path = save(&auth)?;
                println!("登录成功,凭证已写入 {}", path.display());
                return Ok(());
            }
            STATUS_NOT_SCANNED | STATUS_NOT_CONFIRMED => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => return Err(eyre!("二维码登录已取消")),
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
            STATUS_EXPIRED => return Err(eyre!("二维码已过期,请重新执行登录命令")),
            other => return Err(eyre!("未知二维码登录状态码: {other}")),
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

/// 把轮询状态码翻成中文提示(过渡态打 stderr,终态由调用方处理)。
fn print_status_hint(code: i64) {
    match code {
        STATUS_NOT_SCANNED => eprintln!("状态: 等待扫码"),
        STATUS_NOT_CONFIRMED => eprintln!("状态: 等待手机确认"),
        _ => {}
    }
}

/// 从 channel 的 cookie jar 里取出登录凭证三件套。
fn extract_auth(channel: &BilibiliChannel) -> color_eyre::Result<StoredBilibiliAuth> {
    let jar = channel
        .transport()
        .cookie_jar()
        .ok_or_else(|| eyre!("二维码登录后未拿到 cookie jar"))?;
    let uri: Uri = BASE_URL.parse().context("parse bilibili base uri failed")?;
    let get = |name: &str| jar.get_by_name(&uri, name).map(|c| c.value().to_owned());
    let sessdata = get("SESSDATA").ok_or_else(|| eyre!("登录成功但 jar 中无 SESSDATA"))?;
    let bili_jct = get("bili_jct").unwrap_or_default();
    let dede_user_id = get("DedeUserID").ok_or_else(|| eyre!("登录成功但 jar 中无 DedeUserID"))?;
    Ok(StoredBilibiliAuth {
        sessdata,
        bili_jct,
        dede_user_id,
    })
}

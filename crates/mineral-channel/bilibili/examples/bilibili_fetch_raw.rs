// reason: 一次性调试 example,放开与 apitest 一致的 lint。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

//! 拉取 B站任意端点的**原始 JSON**(信封的 `data`),用于排查取流 / 搜索字段问题
//! (如 `dash.audio` 里 `baseUrl`/`base_url` 双键导致反序列化 `duplicate field`)。
//!
//! 复用主程序登录态:`load_stored` 读 `<data_dir>/bilibili.json`(需先
//! `mineral channel bilibili login`);未登录则 guest(音质封顶、私密夹不可见)。
//!
//! ```bash
//! # 搜视频(WBI 签名 + buvid3)
//! cargo run -p mineral-channel-bilibili --example fetch_raw -- search "Chinese Football"
//!
//! # 视频详情(免签)——看分 P / cid
//! cargo run -p mineral-channel-bilibili --example fetch_raw -- view BV1jdTr6aEcL
//!
//! # 取流(自动从 view 取 P1 cid)——排查「卡在开头」就看这个的 dash.audio 结构
//! cargo run -p mineral-channel-bilibili --example fetch_raw -- playurl BV1jdTr6aEcL
//!
//! # WBI keys(nav)
//! cargo run -p mineral-channel-bilibili --example fetch_raw -- nav
//!
//! # 任意端点:URL + 是否 WBI 签名 + 若干 k=v 参数
//! cargo run -p mineral-channel-bilibili --example fetch_raw -- \
//!     get https://api.bilibili.com/x/web-interface/view --plain bvid=BV1jdTr6aEcL
//! ```

use clap::{Parser, Subcommand};
use color_eyre::eyre::eyre;
use mineral_channel_bilibili::{BilibiliChannel, BilibiliConfig, load_stored};

/// 本 example 的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

/// 视频搜索端点(WBI)。
const SEARCH_URL: &str = "https://api.bilibili.com/x/web-interface/wbi/search/type";
/// 视频详情端点(免签)。
const VIEW_URL: &str = "https://api.bilibili.com/x/web-interface/view";
/// 取流端点(WBI)。
const PLAYURL_URL: &str = "https://api.bilibili.com/x/player/wbi/playurl";
/// WBI keys 端点。
const NAV_URL: &str = "https://api.bilibili.com/x/web-interface/nav";

/// 命令行参数。
#[derive(Parser)]
#[command(about = "拉取 B站端点的原始 JSON(调试用)")]
struct Cli {
    /// 子命令。
    #[command(subcommand)]
    cmd: Cmd,
}

/// 支持的子命令。
#[derive(Subcommand)]
enum Cmd {
    /// 搜视频(WBI)。
    Search {
        /// 关键词。
        keyword: String,
    },
    /// 视频详情(免签)。
    View {
        /// BV 号。
        bvid: String,
    },
    /// 取流(自动从 view 取 P1 cid,WBI)。
    Playurl {
        /// BV 号。
        bvid: String,
    },
    /// WBI keys(nav)。
    Nav,
    /// 任意端点:URL + 参数(默认 WBI 签名,`--plain` 走免签)。
    Get {
        /// 完整 URL(不含 query)。
        url: String,

        /// 免签(默认 WBI 签名)。
        #[arg(long)]
        plain: bool,

        /// 若干 `key=value` 参数。
        #[arg(value_name = "KEY=VALUE")]
        params: Vec<String>,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let ch = build_channel()?;
    let t = ch.transport();

    let data = match cli.cmd {
        Cmd::Search { keyword } => {
            t.get_signed(
                SEARCH_URL,
                vec![
                    ("search_type", "video".to_owned()),
                    ("keyword", keyword),
                    ("page", "1".to_owned()),
                    ("order", "totalrank".to_owned()),
                ],
            )
            .await?
        }
        Cmd::View { bvid } => t.get_data(&format!("{VIEW_URL}?bvid={bvid}")).await?,
        Cmd::Playurl { bvid } => {
            let view = t.get_data(&format!("{VIEW_URL}?bvid={bvid}")).await?;
            let cid = view
                .get("cid")
                .and_then(serde_json::Value::as_i64)
                .or_else(|| {
                    view.get("pages")
                        .and_then(|p| p.get(0))
                        .and_then(|p| p.get("cid"))
                        .and_then(serde_json::Value::as_i64)
                })
                .ok_or_else(|| eyre!("view 响应里没找到 cid"))?;
            eprintln!("(P1 cid={cid})");
            t.get_signed(
                PLAYURL_URL,
                vec![
                    ("bvid", bvid),
                    ("cid", cid.to_string()),
                    ("fnval", "4048".to_owned()),
                    ("fourk", "1".to_owned()),
                ],
            )
            .await?
        }
        Cmd::Nav => t.get_data(NAV_URL).await?,
        Cmd::Get { url, plain, params } => {
            let pairs = parse_params(&params)?;
            if plain {
                let query = pairs
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&");
                let full = if query.is_empty() {
                    url
                } else {
                    format!("{url}?{query}")
                };
                t.get_data(&full).await?
            } else {
                t.get_signed(&url, pairs).await?
            }
        }
    };

    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

/// 把 `key=value` 列表解析成 signed 参数对。
fn parse_params(raw: &[String]) -> Result<Vec<(&str, String)>> {
    raw.iter()
        .map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k, v.to_owned()))
                .ok_or_else(|| eyre!("参数须是 key=value:{kv}"))
        })
        .collect()
}

/// 用已存登录态构造 channel;未登录则 guest。
fn build_channel() -> Result<BilibiliChannel> {
    let cfg = BilibiliConfig::builder()
        .max_connections(0)
        .proxy(None)
        .timeout_secs(100)
        .build();
    match load_stored()? {
        Some(auth) => {
            eprintln!("(用已存登录态 mid={})", auth.dede_user_id);
            BilibiliChannel::with_credential(&cfg, &auth)
        }
        None => {
            eprintln!("(未登录:guest 访问,音质封顶 / 私密夹不可见;登录走 channel bilibili login)");
            BilibiliChannel::new(&cfg)
        }
    }
}

// reason: 一次性调试 example,放开与 apitest 一致的 lint。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

//! 拉取网易云任意端点的**原始 JSON**(解密解压后的响应),用于排查字段问题
//! (如某歌单某首歌 `al.name` 为 `null` 导致反序列化失败,先用它看原始数据)。
//!
//! 复用主程序登录态:`load_stored` 读 `<data_dir>/netease.json`,需提前
//! `mineral channel login`;未登录则匿名访问(只能跑公开端点)。
//!
//! ```bash
//! # 歌单详情(便捷子命令)
//! cargo run -p mineral-channel-netease --example fetch_raw -- playlist 5036089714
//!
//! # 任意端点(通用):path + 加密方式(weapi/eapi/linuxapi)+ params(JSON object)
//! cargo run -p mineral-channel-netease --example fetch_raw -- \
//!     raw /weapi/song/enhance/player/url weapi '{"ids":"[1862188922]","br":999000}'
//! cargo run -p mineral-channel-netease --example fetch_raw -- \
//!     raw /api/song/lyric weapi '{"id":"1862188922","lv":-1,"yv":-1,"tv":-1}'
//! cargo run -p mineral-channel-netease --example fetch_raw -- \
//!     raw /weapi/cloudsearch/get/web weapi '{"s":"晴天","type":1,"limit":5}'
//! ```

use clap::{Parser, Subcommand, ValueEnum};
use mineral_channel_netease::transport::client::RequestSpec;
use mineral_channel_netease::transport::headers::UaKind;
use mineral_channel_netease::transport::url::Crypto;
use mineral_channel_netease::{NeteaseChannel, NeteaseConfig, load_stored};

/// 本 example 的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

/// 命令行参数。
#[derive(Parser)]
#[command(about = "拉取网易云端点的原始 JSON(调试用)")]
struct Cli {
    /// 子命令。
    #[command(subcommand)]
    cmd: Cmd,
}

/// 支持的子命令。
#[derive(Subcommand)]
enum Cmd {
    /// 歌单详情(/api/v6/playlist/detail, linuxapi)。
    Playlist {
        /// 歌单 ID。
        id: String,
    },

    /// 任意端点:逻辑 path + 加密方式 + params(JSON object 字符串)。
    Raw {
        /// 逻辑路径,如 `/weapi/song/enhance/player/url`。
        path: String,

        /// 加密方式。
        #[arg(value_enum)]
        crypto: CryptoArg,

        /// 请求参数(JSON object),默认 `{}`。
        #[arg(default_value = "{}")]
        params: String,
    },
}

/// CLI 侧的加密方式枚举,映射到 [`Crypto`]。
#[derive(Clone, Copy, ValueEnum)]
enum CryptoArg {
    /// weapi(网页端加密)。
    Weapi,
    /// eapi(客户端加密)。
    Eapi,
    /// linuxapi(Linux 客户端加密)。
    Linuxapi,
}

impl CryptoArg {
    /// 映射到传输层 [`Crypto`]。
    fn to_crypto(self) -> Crypto {
        match self {
            Self::Weapi => Crypto::Weapi,
            Self::Eapi => Crypto::Eapi,
            Self::Linuxapi => Crypto::Linuxapi,
        }
    }

    /// 选 UA:linuxapi 用 Linux,其余用 PC。
    fn ua(self) -> UaKind {
        match self {
            Self::Linuxapi => UaKind::Linux,
            _ => UaKind::Pc,
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let ch = build_channel()?;

    let (path, crypto, ua, params) = match cli.cmd {
        Cmd::Playlist { id } => {
            let mut p = serde_json::Map::new();
            p.insert("id".into(), serde_json::json!(id));
            p.insert("offset".into(), serde_json::json!("0"));
            p.insert("total".into(), serde_json::json!("true"));
            p.insert("limit".into(), serde_json::json!("1000"));
            p.insert("n".into(), serde_json::json!("1000"));
            (
                "/api/v6/playlist/detail".to_owned(),
                Crypto::Linuxapi,
                UaKind::Linux,
                p,
            )
        }
        Cmd::Raw {
            path,
            crypto,
            params,
        } => {
            let parsed: serde_json::Value = serde_json::from_str(&params)?;
            let map = parsed
                .as_object()
                .ok_or_else(|| color_eyre::eyre::eyre!("params 必须是 JSON object"))?
                .clone();
            (path, crypto.to_crypto(), crypto.ua(), map)
        }
    };

    let resp = ch
        .transport()
        .request_lax(RequestSpec {
            path: &path,
            crypto,
            params,
            ua,
        })
        .await?;
    println!("{}", serde_json::to_string_pretty(&resp)?);
    Ok(())
}

/// 用已存登录态构造 channel;未登录则匿名(仅公开端点可用)。
fn build_channel() -> Result<NeteaseChannel> {
    let cfg = NeteaseConfig::default();
    match load_stored()? {
        Some(auth) => {
            eprintln!("(用已存登录态 user_id={})", auth.user_id.as_str());
            NeteaseChannel::with_credential(&cfg, &auth.music_u, auth.user_id)
        }
        None => {
            eprintln!("(未登录:匿名访问,仅公开端点可用;如需登录态请先 channel login)");
            NeteaseChannel::new(&cfg)
        }
    }
}

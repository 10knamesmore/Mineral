// reason: 联调 example 中常规使用 unwrap / clone / 闭包 map 等,与 crate 主体一致放开。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_closure_for_method_calls,
    clippy::implicit_clone,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::cloned_ref_to_slice_refs
)]

//! 网易云 channel 全面联调。
//!
//! 跑法(用 `cargo apitest` alias):
//! ```bash
//! cargo apitest                          # 只跑无登录部分
//! NETEASE_MUSIC_U=<浏览器 cookie> cargo apitest   # 同时跑登录态用例
//! ```
//!
//! 输出形如:
//! ```text
//! === 1. 无登录联调 ===
//! [✓] WEAPI /weapi/search/hot
//! [✓] LINUXAPI /api/v2/banner/get
//!
//! === 2. 公开数据(MusicChannel API) ===
//! [✓] search_songs           ↳ 5 hits, first: "晴天"
//! [✓] songs_detail           ↳ 1 song
//! [✓] songs_in_album         ↳ 11 tracks
//! [✓] song_urls (Higher)     ↳ url scheme=https, format=mp3
//! [✓] lyrics                 ↳ lrc=Some, yrc=Some
//!
//! === 3. 登录态 ===
//! (跳过:未设 NETEASE_MUSIC_U)
//!
//! === Summary === 7/7 passed
//! ```
//!
//! 失败用例不会 abort 后续——一次跑完看到全部结果。

use std::io::Write;

use mineral_channel_core::{Credential, MusicChannel, Page};
use mineral_channel_netease::transport::client::RequestSpec;
use mineral_channel_netease::transport::headers::UaKind;
use mineral_channel_netease::transport::url::Crypto;
use mineral_channel_netease::{NeteaseChannel, NeteaseConfig};
use mineral_model::{BitRate, MediaUrl};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() -> color_eyre::Result<()> {
    let cookie = std::env::var("NETEASE_MUSIC_U").ok();

    let ch = match cookie.as_deref() {
        Some(c) if !c.is_empty() => {
            println!("(NETEASE_MUSIC_U 已设置,会跑登录态用例)\n");
            NeteaseChannel::with_cookie(&NeteaseConfig::default(), c)?
        }
        _ => NeteaseChannel::new(&NeteaseConfig::default())?,
    };
    let mut report: Vec<(String, std::result::Result<String, String>)> = Vec::new();

    // ---------------- 1. 无登录联调 ----------------
    println!("=== 1. 无登录联调 ===");

    let r = run("WEAPI /weapi/search/hot", async {
        let mut p = serde_json::Map::new();
        p.insert("type".into(), json!("1518"));
        let (code, body) = ch
            .transport()
            .ping(RequestSpec {
                path: "/weapi/search/hot",
                crypto: Crypto::Weapi,
                params: p,
                ua: UaKind::Pc,
            })
            .await?;
        if code != 200 {
            color_eyre::eyre::bail!("code={code}, body={}", truncate(&body.to_string()));
        }
        let n = body
            .get("result")
            .and_then(|x| x.get("hots"))
            .and_then(|x| x.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(format!("{n} hot keywords"))
    })
    .await;
    report.push(r);

    let r = run("LINUXAPI /api/v2/banner/get", async {
        let mut p = serde_json::Map::new();
        p.insert("clientType".into(), json!("pc"));
        let (code, body) = ch
            .transport()
            .ping(RequestSpec {
                path: "/api/v2/banner/get",
                crypto: Crypto::Linuxapi,
                params: p,
                ua: UaKind::Linux,
            })
            .await?;
        if code != 200 {
            color_eyre::eyre::bail!("code={code}, body={}", truncate(&body.to_string()));
        }
        let n = body
            .get("banners")
            .and_then(|x| x.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(format!("{n} banners"))
    })
    .await;
    report.push(r);

    // ---------------- 2. 公开数据(channel API)----------------
    println!("\n=== 2. 公开数据(MusicChannel API)===");

    // 用搜索结果驱动后续所有查询。
    let song_ref = match ch.search_songs("周杰伦", Page::new(0, 5)).await {
        Ok(songs) => {
            let first = songs.first().cloned();
            report.push((
                "search_songs".into(),
                Ok(format!(
                    "{} hits{}",
                    songs.len(),
                    first
                        .as_ref()
                        .map(|s| format!(", first: \"{}\"", s.name))
                        .unwrap_or_default()
                )),
            ));
            print_last(&report);
            first
        }
        Err(e) => {
            report.push(("search_songs".into(), Err(e.to_string())));
            print_last(&report);
            None
        }
    };

    if let Some(song) = song_ref.clone() {
        let r = run("songs_detail", async {
            let v = ch.songs_detail(&[song.id.clone()]).await?;
            Ok(format!("{} songs", v.len()))
        })
        .await;
        report.push(r);

        if let Some(album) = song.album.clone() {
            let r = run("songs_in_album", async {
                let v = ch.songs_in_album(&album.id).await?;
                Ok(format!("{} tracks", v.len()))
            })
            .await;
            report.push(r);
        }

        let r = run("song_urls (Higher)", async {
            let v = ch.song_urls(&[song.id.clone()], BitRate::Higher).await?;
            let u = v.first();
            Ok(match u {
                Some(p) => format!(
                    "1 url, scheme={}, format={}",
                    match &p.url {
                        MediaUrl::Remote(u) => u.scheme(),
                        MediaUrl::Local(_) => "local",
                    },
                    if p.format.is_empty() { "?" } else { &p.format }
                ),
                None => "0 urls (可能需要登录)".into(),
            })
        })
        .await;
        report.push(r);

        let r = run("lyrics", async {
            let l = ch.lyrics(&song.id).await?;
            Ok(format!(
                "lrc={}, yrc={}, translation={}",
                if l.lrc.is_some() { "Some" } else { "None" },
                if l.yrc.is_some() { "Some" } else { "None" },
                if l.translation.is_some() {
                    "Some"
                } else {
                    "None"
                },
            ))
        })
        .await;
        report.push(r);
    } else {
        eprintln!("跳过 songs_detail/album/url/lyrics(因为 search_songs 没产出歌曲)");
    }

    // ---------------- 3. 登录态 ----------------
    println!("\n=== 3. 登录态 ===");
    if cookie.as_deref().filter(|c| !c.is_empty()).is_none() {
        println!("(跳过:未设 NETEASE_MUSIC_U)");
    } else {
        let r = run("login (token refresh)", async {
            ch.login(Credential::Cookie(cookie.clone().unwrap()))
                .await?;
            Ok("refreshed".into())
        })
        .await;
        report.push(r);

        // 注意:UserPlaylistService 需要 uid;为简化,先调 /api/nuser/account/get 拿 uid
        let r = run("user account → uid", async {
            let mut p = serde_json::Map::new();
            let (code, body) = ch
                .transport()
                .ping(RequestSpec {
                    path: "/api/nuser/account/get",
                    crypto: Crypto::Weapi,
                    params: p.clone(),
                    ua: UaKind::Pc,
                })
                .await?;
            let _ = &mut p;
            if code != 200 {
                color_eyre::eyre::bail!("code={code}");
            }
            let uid = body
                .get("account")
                .and_then(|x| x.get("id"))
                .and_then(|x| x.as_i64())
                .ok_or_else(|| color_eyre::eyre::eyre!("missing account.id"))?;
            Ok(format!("uid={uid}"))
        })
        .await;
        let uid_str =
            r.1.as_ref()
                .ok()
                .and_then(|s| s.strip_prefix("uid="))
                .map(str::to_owned);
        report.push(r);

        if let Some(uid) = uid_str {
            let r = run("user_playlists", async {
                let v = ch
                    .user_playlists(&mineral_model::UserId::new(uid.clone()))
                    .await?;
                Ok(format!(
                    "{} playlists{}",
                    v.len(),
                    v.first()
                        .map(|p| format!(", first: \"{}\"", p.name))
                        .unwrap_or_default()
                ))
            })
            .await;
            // 用 first playlist 跑 songs_in_playlist
            let first_playlist = ch
                .user_playlists(&mineral_model::UserId::new(uid.clone()))
                .await
                .ok()
                .and_then(|v| v.into_iter().next());
            report.push(r);

            if let Some(p) = first_playlist {
                let r = run("songs_in_playlist (我喜欢)", async {
                    let v = ch.songs_in_playlist(&p.id).await?;
                    Ok(format!("{} tracks", v.len()))
                })
                .await;
                report.push(r);
            }
        }
    }

    // ---------------- Summary ----------------
    let total = report.len();
    let pass = report.iter().filter(|(_, r)| r.is_ok()).count();
    println!("\n=== Summary === {pass}/{total} passed");
    for (name, r) in &report {
        match r {
            Ok(detail) => println!("  ✓ {name:<35}  {detail}"),
            Err(err) => println!("  ✗ {name:<35}  {err}"),
        }
    }
    if pass < total {
        std::process::exit(1);
    }
    Ok(())
}

async fn run(
    name: &str,
    fut: impl std::future::Future<Output = color_eyre::Result<String>>,
) -> (String, std::result::Result<String, String>) {
    print!("[ ] {name} ... ");
    std::io::stdout().flush().ok();
    let r = fut.await.map_err(|e| e.to_string());
    match &r {
        Ok(d) => println!("\r[✓] {name:<35}  {d}"),
        Err(e) => println!("\r[✗] {name:<35}  {e}"),
    }
    (name.to_owned(), r)
}

fn print_last(report: &[(String, std::result::Result<String, String>)]) {
    if let Some((name, r)) = report.last() {
        match r {
            Ok(d) => println!("[✓] {name:<35}  {d}"),
            Err(e) => println!("[✗] {name:<35}  {e}"),
        }
    }
}

fn truncate(s: &str) -> String {
    if s.len() > 200 {
        format!("{}…", &s[..200])
    } else {
        s.to_owned()
    }
}

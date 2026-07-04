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
//! cargo apitest                          # 优先用本地 netease.json 凭证,无则匿名
//! NETEASE_MUSIC_U=<cookie> cargo apitest  # 无本地 json 时用 env 注入 cookie
//! ```
//!
//! 凭证三级 fallback:
//!   1. 本地 netease.json(完整凭证,带 uid)
//!   2. 环境变量 NETEASE_MUSIC_U(只有 cookie,无 uid)
//!   3. 匿名(无凭证)
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
//! [✓] album_detail           ↳ 11 tracks
//! [✓] song_urls (Higher)     ↳ url scheme=https, format=mp3
//! [✓] lyrics                 ↳ lrc=Some, yrc=Some
//!
//! === 3. 登录态 ===
//! (跳过:无登录凭证)
//!
//! === Summary === 7/7 passed
//! ```
//!
//! 失败用例不会 abort 后续——一次跑完看到全部结果。

use std::io::Write;

use mineral_channel_core::{Credential, MusicChannel, Page};
use mineral_channel_netease::credential::load_stored;
use mineral_channel_netease::transport::client::RequestSpec;
use mineral_channel_netease::transport::headers::UaKind;
use mineral_channel_netease::transport::url::Crypto;
use mineral_channel_netease::{NeteaseChannel, NeteaseConfig};

/// example 用的基线参数(无配置语境,写死;生产默认见 mineral-config 的 default.lua)。
fn netease_config() -> NeteaseConfig {
    NeteaseConfig::builder()
        .max_connections(0)
        .proxy(None)
        .timeout_secs(100)
        .build()
}

use mineral_model::{BitRate, MediaUrl};
use serde_json::json;

/// 凭证来源,用于决定 section 3/4 的行为。
enum CredLevel {
    /// 本地 netease.json,带 uid。
    StoredJson,
    /// 环境变量 NETEASE_MUSIC_U,无 uid。
    EnvCookie,
    /// 匿名,无任何凭证。
    Anonymous,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install().ok();

    // --- 凭证三级 fallback ---
    let (ch, cred_level) = {
        // 1. 本地 netease.json
        if let Some(auth) = load_stored()? {
            println!("凭证: 本地 netease.json (uid={})\n", auth.user_id.as_str());
            let ch = NeteaseChannel::with_credential(
                &netease_config(),
                &auth.music_u,
                auth.user_id,
                mineral_persist::ServerStore::disabled(),
            )?;
            (ch, CredLevel::StoredJson)
        } else {
            let env_cookie = std::env::var("NETEASE_MUSIC_U").ok();
            match env_cookie.as_deref() {
                // 2. 环境变量
                Some(c) if !c.is_empty() => {
                    println!("凭证: 环境变量 NETEASE_MUSIC_U\n");
                    let ch = NeteaseChannel::with_cookie(
                        &netease_config(),
                        c,
                        mineral_persist::ServerStore::disabled(),
                    )?;
                    (ch, CredLevel::EnvCookie)
                }
                // 3. 匿名
                _ => {
                    println!("凭证: 无(匿名)\n");
                    let ch = NeteaseChannel::new(
                        &netease_config(),
                        mineral_persist::ServerStore::disabled(),
                    )?;
                    (ch, CredLevel::Anonymous)
                }
            }
        }
    };

    let has_cookie = !matches!(cred_level, CredLevel::Anonymous);
    let has_uid = matches!(cred_level, CredLevel::StoredJson);

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
        Ok(hits) => {
            let songs = hits.items;
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
            let r = run("album_detail", async {
                let v = ch.album_detail(&album.id).await?;
                Ok(format!("{} tracks", v.songs.len()))
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
                    if p.format.is_empty() {
                        "?"
                    } else {
                        p.format.as_str()
                    }
                ),
                None => "0 urls (可能需要登录)".into(),
            })
        })
        .await;
        report.push(r);

        let r = run("lyrics", async {
            let l = ch.lyrics(&song.id).await?;
            Ok(format!(
                "lines={} ({} 逐字), translated={} lines",
                l.lines.len(),
                l.lines
                    .iter()
                    .filter(|x| !x.kind.words().is_empty())
                    .count(),
                l.lines.iter().filter(|x| x.translation.is_some()).count(),
            ))
        })
        .await;
        report.push(r);
    } else {
        eprintln!("跳过 songs_detail/album/url/lyrics(因为 search_songs 没产出歌曲)");
    }

    // 歌手三连:搜索 → 详情 → 专辑列表(全部匿名可用)
    run_artist_readonly(&ch, &mut report).await;

    // ---------------- 3. 登录态 ----------------
    println!("\n=== 3. 登录态 ===");
    if !has_cookie {
        println!("(跳过:无登录凭证)");
    } else {
        let env_cookie = std::env::var("NETEASE_MUSIC_U").ok();
        let r = run("login (token refresh)", async {
            ch.login(Credential::Cookie(env_cookie.clone().unwrap_or_default()))
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
                    .user_playlists(&mineral_model::UserId::new(
                        mineral_model::SourceKind::NETEASE,
                        uid.clone(),
                    ))
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
                .user_playlists(&mineral_model::UserId::new(
                    mineral_model::SourceKind::NETEASE,
                    uid.clone(),
                ))
                .await
                .ok()
                .and_then(|v| v.into_iter().next());
            report.push(r);

            if let Some(p) = first_playlist {
                let r = run("playlist_detail (我喜欢)", async {
                    let v = ch.playlist_detail(&p.id).await?;
                    Ok(format!("{} tracks", v.songs.len()))
                })
                .await;
                report.push(r);
            }
        }
    }

    // ---------------- 4. Endserenading 歌单原始 JSON ----------------
    println!("\n=== 4. Endserenading 歌单原始 JSON ===");
    run_section4_endserenading(&ch, has_uid, &mut report).await?;

    // ---------------- 5. limit=0 探测(B 方案轻量请求前提)----------------
    println!("\n=== 5. limit=0 探测(对比 tracks 数 / body 大小)===");
    probe_limit_zero(&ch, &mut report).await?;

    // ---------------- 6. 歌单写操作(危险,双重 opt-in)----------------
    println!("\n=== 6. 歌单写操作 ===");
    run_section6_playlist_write(&ch, has_cookie, &mut report).await;

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

/// 歌手只读三连:搜索 → 详情(简介 + 热门曲)→ 专辑列表,全部匿名可用。
async fn run_artist_readonly(
    ch: &NeteaseChannel,
    report: &mut Vec<(String, Result<String, String>)>,
) {
    let artist_ref = match ch.search_artists("Beyond", Page::new(0, 5)).await {
        Ok(hits) => {
            let artists = hits.items;
            let first = artists.first().cloned();
            report.push((
                "search_artists".into(),
                Ok(format!(
                    "{} hits{}",
                    artists.len(),
                    first
                        .as_ref()
                        .map(|a| format!(", first: \"{}\"", a.name))
                        .unwrap_or_default()
                )),
            ));
            print_last(report);
            first
        }
        Err(e) => {
            report.push(("search_artists".into(), Err(e.to_string())));
            print_last(report);
            None
        }
    };

    let Some(artist) = artist_ref else {
        eprintln!("跳过 artist_detail/artist_albums(因为 search_artists 没产出歌手)");
        return;
    };
    let r = run("artist_detail", async {
        let a = ch.artist_detail(&artist.id).await?;
        Ok(format!(
            "\"{}\", {} hot songs, desc {} chars",
            a.name,
            a.songs.len(),
            a.description.chars().count()
        ))
    })
    .await;
    report.push(r);

    let r = run("artist_albums", async {
        let v = ch.artist_albums(&artist.id, Page::new(0, 10)).await?;
        Ok(format!(
            "{} albums{}",
            v.len(),
            v.first()
                .map(|a| format!(", first: \"{}\"", a.name))
                .unwrap_or_default()
        ))
    })
    .await;
    report.push(r);
}

/// 歌单写操作全回路:建单 → 改名 → 改描述 → 加歌 → 删歌 → 删单(自清理)。
///
/// **会真实修改账号数据**,需要双重 opt-in:登录 cookie + `NETEASE_WRITE_TEST=1`。
/// 缺一即跳过——`cargo apitest` 常规跑永远不会碰到写端点。
/// 每步之间 sleep 1s 降低风控(512)概率;若中途失败,残留的临时歌单
/// (名字带 "mineral-apitest" 前缀)需手动去网易云删除。
async fn run_section6_playlist_write(
    ch: &NeteaseChannel,
    has_cookie: bool,
    report: &mut Vec<(String, Result<String, String>)>,
) {
    if !has_cookie || std::env::var("NETEASE_WRITE_TEST").as_deref() != Ok("1") {
        println!("(跳过:写操作需登录 cookie + NETEASE_WRITE_TEST=1 双重 opt-in)");
        return;
    }
    let pause = || tokio::time::sleep(std::time::Duration::from_secs(1));

    let created = match ch.create_playlist("mineral-apitest 临时歌单").await {
        Ok(p) => {
            report.push((
                "create_playlist".into(),
                Ok(format!("id={}", p.id.as_str())),
            ));
            print_last(report);
            p
        }
        Err(e) => {
            report.push(("create_playlist".into(), Err(e.to_string())));
            print_last(report);
            return;
        }
    };
    pause().await;

    let r = run("rename_playlist", async {
        ch.rename_playlist(&created.id, "mineral-apitest 改名后")
            .await?;
        Ok("renamed".into())
    })
    .await;
    report.push(r);
    pause().await;

    let r = run("set_playlist_description", async {
        ch.set_playlist_description(&created.id, "apitest 自动创建,跑完即删")
            .await?;
        Ok("desc updated".into())
    })
    .await;
    report.push(r);
    pause().await;

    // 拿一首公开歌做加/删素材
    let donor = ch
        .search_songs("海阔天空", Page::new(0, 1))
        .await
        .ok()
        .and_then(|hits| hits.items.into_iter().next());
    if let Some(song) = donor {
        let r = run("playlist_add_songs", async {
            ch.playlist_add_songs(&created.id, std::slice::from_ref(&song.id))
                .await?;
            Ok(format!("added \"{}\"", song.name))
        })
        .await;
        report.push(r);
        pause().await;

        let r = run("playlist_add_songs (重复→502)", async {
            match ch
                .playlist_add_songs(&created.id, std::slice::from_ref(&song.id))
                .await
            {
                Err(mineral_channel_core::Error::Api { code: 502, .. }) => {
                    Ok("dup correctly rejected with 502".into())
                }
                Err(e) => color_eyre::eyre::bail!("expected Api 502, got: {e}"),
                Ok(()) => color_eyre::eyre::bail!("expected Api 502, got Ok"),
            }
        })
        .await;
        report.push(r);
        pause().await;

        let r = run("playlist_remove_songs", async {
            ch.playlist_remove_songs(&created.id, std::slice::from_ref(&song.id))
                .await?;
            Ok("removed".into())
        })
        .await;
        report.push(r);
        pause().await;
    } else {
        eprintln!("跳过加/删歌(search_songs 没产出素材)");
    }

    let r = run("delete_playlist", async {
        ch.delete_playlist(&created.id).await?;
        Ok("cleaned up".into())
    })
    .await;
    report.push(r);
}

/// 写死的 "Endserenading" 歌单 id(用户自己的私有歌单,基本不动)。
///
/// 运行时**直接 ping 这个固定 id**,不查本地 DB、不拉 `my_playlists` 列表
/// (那样会依赖特定用户/运行时状态,别人跑就全 fail)。来源:
/// `https://music.163.com/#/playlist?id=8411923778`。
const ENDSERENADING_PLAYLIST_ID: &str = "8411923778";

/// 探测用的公开歌单(网易云飙升榜,100 首,无需登录),用于对比 limit=0 vs limit=1000。
const PROBE_PUBLIC_PLAYLIST_ID: &str = "19723756";

/// 探测 `limit=0` 是否省掉 `tracks` 大头(B 方案"轻量版本请求"的前提)。
///
/// 对同一公开歌单分别用 `limit=0` / `limit=1000` 打 `/api/v6/playlist/detail`,
/// 打印各自的 `tracks` 数、`trackIds` 数、`trackUpdateTime`、body 字节数。
/// 判读:若 `limit=0` 那行 `tracks=0` 且 `trackIds` 仍全量、body 远小于 `limit=1000`,
/// 则 `limit=0` 有效(轻请求确实省掉了完整曲目大头)。
///
/// # Params:
///   - `ch`: 网易云 channel
///   - `report`: 汇总表
async fn probe_limit_zero(
    ch: &NeteaseChannel,
    report: &mut Vec<(String, std::result::Result<String, String>)>,
) -> color_eyre::Result<()> {
    for limit in ["0", "1000"] {
        let mut p = serde_json::Map::new();
        p.insert("id".into(), json!(PROBE_PUBLIC_PLAYLIST_ID));
        p.insert("offset".into(), json!("0"));
        p.insert("total".into(), json!("true"));
        p.insert("limit".into(), json!(limit));
        p.insert("n".into(), json!(limit));
        let (code, body) = ch
            .transport()
            .ping(RequestSpec {
                path: "/api/v6/playlist/detail",
                crypto: Crypto::Linuxapi,
                params: p,
                ua: UaKind::Linux,
            })
            .await?;
        let label = format!("probe limit={limit}");
        if code != 200 {
            let msg = format!("code={code}");
            println!("[✗] {label:<20}  {msg}");
            report.push((label, Err(msg)));
            continue;
        }
        let playlist = body.get("playlist");
        let arr_len = |key: &str| {
            playlist
                .and_then(|x| x.get(key))
                .and_then(|x| x.as_array())
                .map(|a| a.len())
                .unwrap_or(0)
        };
        let tut = playlist
            .and_then(|x| x.get("trackUpdateTime"))
            .and_then(|x| x.as_i64())
            .unwrap_or(-1);
        let detail = format!(
            "tracks={}, trackIds={}, trackUpdateTime={tut}, body={}B",
            arr_len("tracks"),
            arr_len("trackIds"),
            body.to_string().len()
        );
        println!("[✓] {label:<20}  {detail}");
        report.push((label, Ok(detail)));
    }
    println!(
        "→ 若 limit=0 那行 tracks=0 且 trackIds 仍全量、body 远小于 limit=1000,则 limit=0 有效(省 tracks)"
    );
    Ok(())
}

/// Section 4:对写死的 Endserenading 歌单打 playlist detail + 第一首 song detail,
/// 各自 print 完整原始 JSON。私有歌单需登录凭证(带 uid)。
async fn run_section4_endserenading(
    ch: &NeteaseChannel,
    has_uid: bool,
    report: &mut Vec<(String, std::result::Result<String, String>)>,
) -> color_eyre::Result<()> {
    if !has_uid {
        println!("(跳过:需登录凭证——Endserenading 是私有歌单)");
        return Ok(());
    }

    // --- playlist detail 原始 JSON(写死 id,运行时不碰 DB)---
    let mut p = serde_json::Map::new();
    p.insert("id".into(), json!(ENDSERENADING_PLAYLIST_ID));
    p.insert("offset".into(), json!("0"));
    p.insert("total".into(), json!("true"));
    p.insert("limit".into(), json!("1000"));
    p.insert("n".into(), json!("1000"));
    let (code, body) = ch
        .transport()
        .ping(RequestSpec {
            path: "/api/v6/playlist/detail",
            crypto: Crypto::Linuxapi,
            params: p,
            ua: UaKind::Linux,
        })
        .await?;
    if code != 200 {
        let msg = format!("code={code}, body={}", truncate(&body.to_string()));
        println!("[✗] Endserenading: playlist_detail_v6  {msg}");
        report.push(("Endserenading: playlist_detail_v6".into(), Err(msg)));
        return Ok(());
    }
    println!("\n--- playlist detail 原始 JSON ---");
    println!("{}", serde_json::to_string_pretty(&body)?);
    let track_count = body
        .get("playlist")
        .and_then(|x| x.get("trackCount"))
        .and_then(|x| x.as_i64())
        .unwrap_or(-1);
    report.push((
        "Endserenading: playlist_detail_v6".into(),
        Ok(format!("trackCount={track_count}")),
    ));

    // --- 取第一首歌 id(trackIds[0].id,退路 tracks[0].id)---
    let first_song_id = body
        .get("playlist")
        .and_then(|x| x.get("trackIds"))
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.get("id"))
        .and_then(|x| x.as_i64())
        .or_else(|| {
            body.get("playlist")
                .and_then(|x| x.get("tracks"))
                .and_then(|x| x.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.get("id"))
                .and_then(|x| x.as_i64())
        })
        .map(|id| id.to_string());

    // --- 第一首 song detail 原始 JSON ---
    let Some(sid) = first_song_id else {
        println!("(未能取到第一首歌 id,跳过 song detail)");
        return Ok(());
    };
    let c = vec![json!({ "id": sid })];
    let mut p = serde_json::Map::new();
    p.insert("c".into(), json!(serde_json::to_string(&c)?));
    let (code, body) = ch
        .transport()
        .ping(RequestSpec {
            path: "/weapi/v3/song/detail",
            crypto: Crypto::Weapi,
            params: p,
            ua: UaKind::Any,
        })
        .await?;
    if code != 200 {
        let msg = format!("code={code}, body={}", truncate(&body.to_string()));
        println!("[✗] Endserenading: song_detail_v3  {msg}");
        report.push(("Endserenading: song_detail_v3".into(), Err(msg)));
        return Ok(());
    }
    println!("\n--- 第一首 song detail 原始 JSON ---");
    println!("{}", serde_json::to_string_pretty(&body)?);
    let name = body
        .get("songs")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|s| s.get("name"))
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    report.push((
        "Endserenading: song_detail_v3".into(),
        Ok(format!("name=\"{name}\"")),
    ));
    Ok(())
}

/// 跑一项测试:打印「执行中」前缀,await 完成后回写 `[✓]`/`[✗]` 行,返回 (name, result)。
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

/// 把 report 末尾一条结果按统一格式重打到 stdout(用于离线/失败汇总后的回放)。
fn print_last(report: &[(String, std::result::Result<String, String>)]) {
    if let Some((name, r)) = report.last() {
        match r {
            Ok(d) => println!("[✓] {name:<35}  {d}"),
            Err(e) => println!("[✗] {name:<35}  {e}"),
        }
    }
}

/// 把过长的样本字符串裁到 200 字符 + `…`,避免输出炸屏。
fn truncate(s: &str) -> String {
    if s.len() > 200 {
        format!("{}…", &s[..200])
    } else {
        s.to_owned()
    }
}

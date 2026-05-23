//! 歌词端点(spec §4.4):合并 LRC(`/api/song/lyric`,linuxapi)和
//! YRC(`/api/song/lyric/v1`,eapi)两次调用。

use mineral_model::{LrcLyric, Lyrics, SongId};
use serde_json::{Value, json};

use crate::api::yrc;
use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;

/// 从响应里取 `<key>.lyric` 的字符串(网易把各类歌词都套在 `{ "lyric": "..." }` 下)。
fn lyric_text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| x.get("lyric"))
        .and_then(Value::as_str)
}

pub async fn lyrics(transport: &Transport, id: &SongId) -> color_eyre::Result<Lyrics> {
    let mut out = Lyrics::default();

    // ---- 旧版 LyricService(linuxapi)拿 LRC + 翻译 ----
    let mut p = serde_json::Map::new();
    p.insert("id".into(), json!(id.as_str()));
    p.insert("lv".into(), json!("-1"));
    p.insert("kv".into(), json!("-1"));
    p.insert("tv".into(), json!("-1"));
    if let Ok(v) = transport
        .request(RequestSpec {
            path: "/api/song/lyric",
            crypto: Crypto::Linuxapi,
            params: p,
            ua: UaKind::Linux,
        })
        .await
    {
        if let Some(s) = lyric_text(&v, "lrc") {
            out.lrc = LrcLyric::parse(s);
        }
        if let Some(s) = lyric_text(&v, "tlyric") {
            out.translation = LrcLyric::parse(s);
        }
    }

    // ---- /api/song/lyric/v1(eapi)拿 YRC + 翻译/罗马音 ----
    let mut p = serde_json::Map::new();
    p.insert("id".into(), json!(id.as_str()));
    p.insert("cp".into(), json!("false"));
    for k in ["tv", "lv", "rv", "kv", "yv", "ytv", "yrv"] {
        p.insert(k.into(), json!("0"));
    }
    if let Ok(v) = transport
        .request(RequestSpec {
            path: "/api/song/lyric/v1",
            crypto: Crypto::Eapi,
            params: p,
            ua: UaKind::Mobile,
        })
        .await
    {
        if let Some(s) = lyric_text(&v, "yrc") {
            out.words = yrc::parse_yrc(s).into();
        }
        // v1 的 lrc 若有则覆盖 linuxapi 版。
        if let Some(s) = lyric_text(&v, "lrc") {
            out.lrc = LrcLyric::parse(s);
        }
        // 翻译 / 罗马音都是行级(译文无法逐音节对齐):逐字版 `ytlrc` / `yromalrc` 优先,
        // 退回行级 `tlyric` / `romalrc`。
        if let Some(s) = lyric_text(&v, "ytlrc").or_else(|| lyric_text(&v, "tlyric")) {
            out.translation = LrcLyric::parse(s);
        }
        if let Some(s) = lyric_text(&v, "yromalrc").or_else(|| lyric_text(&v, "romalrc")) {
            out.romanization = LrcLyric::parse(s);
        }
    }

    Ok(out)
}

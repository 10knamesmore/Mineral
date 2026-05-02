//! 歌词端点(spec §4.4):合并 LRC(`/api/song/lyric`,linuxapi)和
//! YRC(`/api/song/lyric/v1`,eapi)两次调用。

use mineral_model::{Lyrics, SongId};
use serde_json::json;

type Result<T> = color_eyre::Result<T>;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;

pub async fn lyrics(transport: &Transport, id: &SongId) -> Result<Lyrics> {
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
        out.lrc = v
            .get("lrc")
            .and_then(|x| x.get("lyric"))
            .and_then(|x| x.as_str())
            .map(str::to_owned);
        out.translation = v
            .get("tlyric")
            .and_then(|x| x.get("lyric"))
            .and_then(|x| x.as_str())
            .map(str::to_owned);
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
        out.yrc = v
            .get("yrc")
            .and_then(|x| x.get("lyric"))
            .and_then(|x| x.as_str())
            .map(str::to_owned);
        out.yrc_translation = v
            .get("ytlrc")
            .and_then(|x| x.get("lyric"))
            .and_then(|x| x.as_str())
            .map(str::to_owned);
        out.yrc_romanization = v
            .get("yromalrc")
            .and_then(|x| x.get("lyric"))
            .and_then(|x| x.as_str())
            .map(str::to_owned);
        // 若 v1 也带 lrc / tlyric,以 v1 为准覆盖
        if let Some(s) = v
            .get("lrc")
            .and_then(|x| x.get("lyric"))
            .and_then(|x| x.as_str())
        {
            out.lrc = Some(s.to_owned());
        }
        if let Some(s) = v
            .get("tlyric")
            .and_then(|x| x.get("lyric"))
            .and_then(|x| x.as_str())
        {
            out.translation = Some(s.to_owned());
        }
        if let Some(s) = v
            .get("romalrc")
            .and_then(|x| x.get("lyric"))
            .and_then(|x| x.as_str())
        {
            out.romanization = Some(s.to_owned());
        }
    }

    Ok(out)
}

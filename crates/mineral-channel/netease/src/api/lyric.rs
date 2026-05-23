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

#[cfg(test)]
mod tests {
    use mineral_model::LrcLyric;

    /// 行级歌词第一行的 `(time_ms, text)`,断言用。
    fn first(lyric: &LrcLyric) -> Option<(u64, &str)> {
        lyric.first().map(|l| (l.time_ms, l.text.as_str()))
    }

    // ───────────────── 真实数据 ─────────────────
    // 用真打网易云存下来的原始文本(`lyric_fixtures/*`)验证「宽进严出」清洗,
    // 也让人一眼看到网易行级歌词的几种真实样式。`LrcLyric::parse` 负责:
    //   - 跳过开头的 credits JSON 行(`{"t":..,"c":[..]}`)与 `[by:..]` 等元 tag;
    //   - 兼容时间戳的点 / 冒号两种分隔符、2~3 位小数;
    //   - 保留空行歌词([mm:ss.xx] 后无文本)。

    #[test]
    fn real_lrc_dot_format() {
        // Mineral《The Last Word Is Rejoice》—— 标准点格式 [mm:ss.xx]。
        let lyric = LrcLyric::parse(include_str!("lyric_fixtures/rejoice.lrc"));
        // 两行 credits JSON 跳过,首行是真正的歌词。
        assert_eq!(
            first(&lyric),
            Some((91_430, "How will I drink from that stream"))
        );
    }

    #[test]
    fn real_lrc_colon_variant() {
        // ひとひら《The Sound of Summer Coming》—— 网易把厘秒也用冒号分隔的变体
        // [01:08:30],曾导致 TUI 解析不出歌词。清洗后应等价于 [01:08.30] = 68300ms。
        let lyric = LrcLyric::parse(include_str!("lyric_fixtures/hitohira.lrc"));
        assert_eq!(
            first(&lyric),
            Some((68_300, "迷子も2人でいれば散歩みたいね"))
        );
        // 严出:序列化回标准点格式。
        assert!(lyric.to_lrc_string().starts_with("[01:08.30]迷子"));
    }

    #[test]
    fn real_lrc_pure_no_translation() {
        // 晴天霹雳(Chinese Football)—— 纯 lrc,无翻译无逐字,且含空行歌词。
        let lyric = LrcLyric::parse(include_str!("lyric_fixtures/qingtian.lrc"));
        // 第一行 [00:04.08] 后无文本,是真实存在的空行(分隔/前奏)。
        assert_eq!(first(&lyric), Some((4_080, "")));
        // 后面是有词的行。
        assert!(lyric.iter().any(|l| l.text == "分不清 朝晖夕阴"));
    }

    #[test]
    fn real_lrc_no_credits_header() {
        // 小河(祝游会)—— 没有 credits 头,首行直接是歌词。
        let lyric = LrcLyric::parse(include_str!("lyric_fixtures/xiaohe.lrc"));
        assert_eq!(first(&lyric), Some((690, "越过一条小河")));
    }

    #[test]
    fn real_translation_skips_by_tag() {
        // 翻译里开头有 [by:..] 署名 tag,不是时间戳,应被跳过。
        let lyric = LrcLyric::parse(include_str!("lyric_fixtures/rejoice.tlyric"));
        assert_eq!(first(&lyric), Some((91_430, "我将如何饮用溪间流水")));
    }

    #[test]
    fn real_romanization_three_digit_millis() {
        // 罗马音用点格式 3 位毫秒 [01:08.300] —— 300ms 应原样,不被当成厘秒。
        let lyric = LrcLyric::parse(include_str!("lyric_fixtures/hitohira.romalrc"));
        assert_eq!(
            first(&lyric),
            Some((68_300, "ma i go mo fu ta ri de i re ba sa n po mi ta i ne"))
        );
    }
}

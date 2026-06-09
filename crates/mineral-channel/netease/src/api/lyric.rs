//! 歌词端点(spec §4.4):合并 LRC(`/api/song/lyric`,linuxapi)和
//! YRC(`/api/song/lyric/v1`,eapi)两次调用,装配成一条统一行序列。

use mineral_model::{LyricLine, Lyrics, SongId, parse_lrc};
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

/// 拉取并装配一首歌的歌词。
///
/// 原文取舍:逐字 yrc 在则用 yrc(自带 credits 纯文本行);否则取两路 lrc 中**行数更多**
/// 的一份(v1 的 JSON 行格式解析等价于 linuxapi 的标准格式,取多者防坏数据覆盖好数据)。
/// 翻译 / 罗马音是行级,逐字版 `ytlrc` / `yromalrc` 优先,退回 `tlyric` / `romalrc`。
///
/// # Params:
///   - `transport`: 已配置加密的传输层
///   - `id`: 歌曲 ID
///
/// # Return:
///   装配好的 [`Lyrics`];任一路失败按缺省(空)处理。
pub async fn lyrics(transport: &Transport, id: &SongId) -> color_eyre::Result<Lyrics> {
    let mut out = Lyrics::default();
    let mut lrc_linux = Vec::<LyricLine>::new();
    let mut lrc_v1 = Vec::<LyricLine>::new();
    let mut yrc_lines = Vec::<LyricLine>::new();

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
            lrc_linux = parse_lrc(s);
        }
        if let Some(s) = lyric_text(&v, "tlyric") {
            out.translation = parse_lrc(s);
        }
    }

    // ---- /api/song/lyric/v1(eapi)拿 YRC + lrc + 翻译/罗马音 ----
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
            yrc_lines = yrc::parse_yrc(s);
        }
        if let Some(s) = lyric_text(&v, "lrc") {
            lrc_v1 = parse_lrc(s);
        }
        if let Some(s) = lyric_text(&v, "ytlrc").or_else(|| lyric_text(&v, "tlyric")) {
            out.translation = parse_lrc(s);
        }
        if let Some(s) = lyric_text(&v, "yromalrc").or_else(|| lyric_text(&v, "romalrc")) {
            out.romanization = parse_lrc(s);
        }
    }

    // 装配原文:yrc 优先(含 credits);否则取行数更多的 lrc(防坏覆盖好)。
    out.original = if !yrc_lines.is_empty() {
        yrc_lines
    } else if lrc_v1.len() >= lrc_linux.len() {
        lrc_v1
    } else {
        lrc_linux
    };

    Ok(out)
}

#[cfg(test)]
mod tests {
    use mineral_model::{LyricLine, parse_lrc, to_lrc_string};

    /// 首行的 `(time_ms, 文本)`,断言用。
    fn first(lines: &[LyricLine]) -> Option<(Option<u64>, String)> {
        lines
            .first()
            .map(|l| (l.time_ms, l.kind.text().into_owned()))
    }

    /// 按文本内容找到对应行的 `time_ms`(对 credits 前置不敏感)。
    fn time_of(lines: &[LyricLine], text: &str) -> Option<Option<u64>> {
        lines
            .iter()
            .find(|l| l.kind.text().as_ref() == text)
            .map(|l| l.time_ms)
    }

    // ───────────────── 真实数据 ─────────────────
    // 用真打网易云存下来的原始文本(`lyric_fixtures/*`)验证「宽进严出」清洗,也让人一眼看到
    // 网易行级歌词的几种真实样式。`parse_lrc` 负责:
    //   - 把开头的 credits JSON 行(`{"t":..,"c":[..]}`)**保留**成带时间戳的纯文本行;
    //   - 跳过 `[by:..]` 等元 tag;
    //   - 兼容时间戳的点 / 冒号两种分隔符、2~3 位小数;保留空行歌词([mm:ss.xx] 后无文本)。

    #[test]
    fn real_lrc_keeps_credits_then_lyrics() {
        // rejoice.lrc 开头是 JSON credits(作词/作曲),现在保留为带时间戳的 Plain 行;正文随后。
        let lines = parse_lrc(include_str!("lyric_fixtures/rejoice.lrc"));
        assert_eq!(first(&lines), Some((Some(0), "作词: Mineral".to_owned())));
        assert_eq!(
            time_of(&lines, "How will I drink from that stream"),
            Some(Some(91_430))
        );
    }

    #[test]
    fn real_lrc_colon_variant() {
        // ひとひら《The Sound of Summer Coming》—— 网易把厘秒也用冒号分隔的变体
        // [01:08:30],曾导致 TUI 解析不出歌词。清洗后应等价于 [01:08.30] = 68300ms。
        let lines = parse_lrc(include_str!("lyric_fixtures/hitohira.lrc"));
        assert_eq!(
            time_of(&lines, "迷子も2人でいれば散歩みたいね"),
            Some(Some(68_300))
        );
        // 严出:序列化回标准点格式(含该行)。
        assert!(to_lrc_string(&lines).contains("[01:08.30]迷子"));
    }

    #[test]
    fn real_lrc_pure_no_translation() {
        // 晴天霹雳(Chinese Football)—— 纯 lrc,无翻译无逐字,且含空行歌词。
        let lines = parse_lrc(include_str!("lyric_fixtures/qingtian.lrc"));
        // [00:04.08] 后无文本,是真实存在的空行(分隔/前奏),应保留。
        assert!(
            lines
                .iter()
                .any(|l| l.time_ms == Some(4_080) && l.kind.text().is_empty()),
            "空行歌词保留"
        );
        // 后面有有词的行(带时间戳)。
        assert!(matches!(time_of(&lines, "分不清 朝晖夕阴"), Some(Some(_))));
    }

    #[test]
    fn real_lrc_no_credits_header() {
        // 小河(祝游会)—— 没有 credits 头,首行直接是歌词。
        let lines = parse_lrc(include_str!("lyric_fixtures/xiaohe.lrc"));
        assert_eq!(first(&lines), Some((Some(690), "越过一条小河".to_owned())));
    }

    #[test]
    fn real_translation_skips_by_tag() {
        // 翻译里开头有 [by:..] 署名 tag,不是时间戳,应被跳过。
        let lines = parse_lrc(include_str!("lyric_fixtures/rejoice.tlyric"));
        assert_eq!(
            first(&lines),
            Some((Some(91_430), "我将如何饮用溪间流水".to_owned()))
        );
    }

    #[test]
    fn real_romanization_three_digit_millis() {
        // 罗马音用点格式 3 位毫秒 [01:08.300] —— 300ms 应原样,不被当成厘秒。
        let lines = parse_lrc(include_str!("lyric_fixtures/hitohira.romalrc"));
        assert_eq!(
            time_of(&lines, "ma i go mo fu ta ri de i re ba sa n po mi ta i ne"),
            Some(Some(68_300))
        );
    }
}

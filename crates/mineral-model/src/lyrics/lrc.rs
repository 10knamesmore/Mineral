//! 行级歌词的解析(宽进)、序列化(严出)与按时间定位。
//!
//! LRC 是跨 channel 的事实标准,所以解析能力放在 model 共享:各 channel 把拿到的原始
//! 歌词文本喂进 [`parse_lrc`] 完成「反序列化 + 清洗」,得到一条 [`LyricLine`] 序列;要交给
//! 外部系统(如 MPRIS `xesam:asText`)时用 [`to_lrc_string`] 重新序列化成标准 LRC。
//!
//! 宽进:同时认三种行格式——标准 `[mm:ss.xx]text`、富文本 JSON 行
//! `{"t":ms,"c":[{"tx":..}]}`、裸文本(无时间戳);无有效时间戳的行整行原样保留(只跳已知
//! meta tag)。严出([`to_lrc_string`]):只吐带时间戳的行,保持标准 LRC 格式。

use serde::Deserialize;

use super::types::{LineKind, LyricLine};

/// 解析一段歌词文本为一条 [`LyricLine`] 序列。
///
/// 按文档序逐行解析,带时间戳的行按时间稳定排序、无时间戳的行保持在其前一带时间戳行之后
/// (排序键继承上一时间戳),使前奏白 / credits / 尾注的相对位置不被打乱。
///
/// # Params:
///   - `s`: 原始歌词文本(标准 LRC / JSON 富文本行 / 裸文本,可混排)。
///
/// # Return:
///   清洗后的行序列。
pub fn parse_lrc(s: &str) -> Vec<LyricLine> {
    let mut keyed = Vec::<(u64, LyricLine)>::new();
    let mut last_time: u64 = 0;
    for raw in s.lines() {
        if raw.trim_start().starts_with('{') {
            if let Some(line) = parse_json_line(raw.trim_start()) {
                let key = line.time_ms.unwrap_or(last_time);
                if let Some(t) = line.time_ms {
                    last_time = t;
                }
                keyed.push((key, line));
            }
            continue;
        }
        parse_text_line(raw, &mut last_time, &mut keyed);
    }
    keyed.sort_by_key(|(k, _)| *k);
    keyed.into_iter().map(|(_, line)| line).collect()
}

/// 序列化成标准 LRC 字符串(严出,厘秒精度 `[mm:ss.xx]text`)。
///
/// **只序列化带时间戳的行**——无时间戳行跳过,使外部消费者(MPRIS `xesam:asText`)看到的
/// 永远是标准 LRC 格式。空 / 全无时间戳返回空串。
///
/// # Params:
///   - `lines`: 行序列。
///
/// # Return:
///   逐行 `[mm:ss.xx]text`、以 `\n` 连接的标准 LRC。
pub fn to_lrc_string(lines: &[LyricLine]) -> String {
    lines
        .iter()
        .filter_map(|l| {
            l.time_ms
                .map(|t| format_lrc_line(t, l.kind.text().as_ref()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 找 `position_ms` 对应的当前行 index(只看带时间戳的行)。
///
/// 返回 `time_ms <= position_ms` 中时间最大者的行 index;无满足者(前奏 / 全无时间戳)返回
/// `None`。
///
/// # Params:
///   - `lines`: 行序列。
///   - `position_ms`: 播放位置(绝对毫秒)。
///
/// # Return:
///   当前行在 `lines` 中的 index。
pub fn current_line(lines: &[LyricLine], position_ms: u64) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| l.time_ms.filter(|t| *t <= position_ms).map(|t| (i, t)))
        .max_by_key(|(_, t)| *t)
        .map(|(i, _)| i)
}

/// 是否有任一带时间戳的行(决定能否自动跟随 / 标题 `synced` 档)。
pub fn has_timed(lines: &[LyricLine]) -> bool {
    lines.iter().any(|l| l.time_ms.is_some())
}

/// 是否有任一逐字行(决定标题 `synced ✦` 档)。
pub fn has_words(lines: &[LyricLine]) -> bool {
    lines.iter().any(|l| !l.kind.words().is_empty())
}

/// 解析 JSON 富文本行 `{"t":ms,"c":[{"tx":..},..]}`:拼接各 `tx` 为文本,`t` 为时间戳
/// (缺省 / 0 时仍按字面)。`c` 为空或拼接后为空白则返回 `None`(跳过)。
fn parse_json_line(line: &str) -> Option<LyricLine> {
    /// 一个文本片段(只取 `tx`,丢弃 `li`/`or` 等链接元数据)。
    #[derive(Deserialize)]
    struct Seg {
        #[serde(default)]
        tx: String,
    }

    /// 一个富文本行。
    #[derive(Deserialize)]
    struct JsonLine {
        #[serde(default)]
        t: Option<u64>,

        #[serde(default)]
        c: Vec<Seg>,
    }

    let parsed = serde_json::from_str::<JsonLine>(line).ok()?;
    if parsed.c.is_empty() {
        return None;
    }
    let text = parsed
        .c
        .iter()
        .map(|s| s.tx.as_str())
        .collect::<String>()
        .trim()
        .to_owned();
    if text.is_empty() {
        return None;
    }
    Some(LyricLine {
        time_ms: parsed.t,
        kind: LineKind::Plain(text),
    })
}

/// 解析一行非 JSON 文本:剥前缀时间戳后,带戳行按各戳展开,无戳行整行原样保留(已知 meta
/// tag / 空行跳过)。
fn parse_text_line(raw: &str, last_time: &mut u64, out: &mut Vec<(u64, LyricLine)>) {
    let mut rest = raw.trim_start();
    let mut stamps = Vec::<u64>::new();

    while rest.starts_with('[') {
        let Some(close) = rest.find(']') else {
            break;
        };
        let Some(inside) = rest.get(1..close) else {
            break;
        };
        let Some(after) = rest.get(close + 1..) else {
            break;
        };
        match parse_timestamp(inside) {
            Some(ms) => {
                stamps.push(ms);
                rest = after.trim_start();
            }
            // 第一个非时间戳括号:停,余下整体交给文本 / meta 判定。
            None => break,
        }
    }

    let text = rest.trim();
    if !stamps.is_empty() {
        for ms in &stamps {
            out.push((*ms, LyricLine::timed(*ms, text)));
            *last_time = (*last_time).max(*ms);
        }
        return;
    }
    if text.is_empty() || is_meta_line(rest) {
        return;
    }
    out.push((*last_time, LyricLine::untimed(text)));
}

/// 整行是否是已知 LRC meta tag(`[ti:]`/`[ar:]`/`[al:]`/`[by:]`/`[offset:]` 等),是则整
/// 行跳过(非歌词内容)。非 meta 的 `[..]`(如 `[Verse]`)不算,留作文本。
fn is_meta_line(s: &str) -> bool {
    let Some(close) = s.find(']') else {
        return false;
    };
    if !s.starts_with('[') {
        return false;
    }
    let Some(inside) = s.get(1..close) else {
        return false;
    };
    let Some(colon) = inside.find(':') else {
        return false;
    };
    let Some(key) = inside.get(..colon) else {
        return false;
    };
    matches!(
        key.to_ascii_lowercase().as_str(),
        "ti" | "ar" | "al" | "au" | "by" | "offset" | "length" | "re" | "ve" | "lang" | "kana"
    )
}

/// 解析 `mm:ss.xx` / `mm:ss.xxx` / `mm:ss:xx`(冒号厘秒变体) / `mm:ss`(无小数)时间戳。
/// 非时间戳(元数据 / 普通括号)返回 `None`。
fn parse_timestamp(s: &str) -> Option<u64> {
    if !s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let colon = s.find(':')?;
    let mm: u64 = s.get(..colon)?.parse().ok()?;
    let after = s.get(colon + 1..)?;

    let (sec_part, ms_part) = match after.find(['.', ':']) {
        Some(sep) => (after.get(..sep)?, after.get(sep + 1..)?),
        None => (after, ""),
    };
    let ss: u64 = sec_part.parse().ok()?;
    let ms: u64 = if ms_part.is_empty() {
        0
    } else {
        let truncated = ms_part.get(..ms_part.len().min(3))?;
        let raw: u64 = truncated.parse().ok()?;
        match truncated.len() {
            1 => raw * 100,
            2 => raw * 10,
            3 => raw,
            _ => return None,
        }
    };
    Some(mm * 60_000 + ss * 1000 + ms)
}

/// 毫秒 + 内容 → 标准 LRC 行 `[mm:ss.xx]内容`(厘秒精度)。
fn format_lrc_line(ms: u64, content: &str) -> String {
    let minutes = ms / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    let centis = (ms % 1_000) / 10;
    format!("[{minutes:02}:{seconds:02}.{centis:02}]{content}")
}

#[cfg(test)]
mod tests {
    use super::super::types::{LineKind, LyricLine, Word};
    use super::{current_line, has_timed, has_words, parse_lrc, to_lrc_string};

    /// 带时间戳文本行(断言构造器)。
    fn timed(time_ms: u64, text: &str) -> LyricLine {
        LyricLine::timed(time_ms, text)
    }

    /// 无时间戳文本行(断言构造器)。
    fn plain(text: &str) -> LyricLine {
        LyricLine::untimed(text)
    }

    #[test]
    fn parses_basic_lrc() {
        let s = "[00:01.00]hello\n[00:02.50]world\n[01:00.123]end";
        assert_eq!(
            parse_lrc(s),
            vec![
                timed(1000, "hello"),
                timed(2500, "world"),
                timed(60_123, "end")
            ]
        );
    }

    /// 真实复杂 LRC(metadata 跳过 + 多时间戳展开 + CJK + 厘秒进位)解析出的整结构快照。
    #[test]
    fn parses_realistic_lrc_snapshot() {
        let s = "[ti:春日影]\n[ar:MyGO!!!!!]\n\
                 [00:00.00]迷星叫\n\
                 [00:12.50]壱雫空\n\
                 [00:15.20][00:48.30]碧天伴走\n\
                 [01:00.999]名無声";
        mineral_test::assert_snap_debug!(
            "真实 LRC:metadata 跳过、多时间戳展开、CJK、厘秒",
            parse_lrc(s)
        );
    }

    #[test]
    fn parses_colon_centisecond_variant() {
        assert_eq!(parse_lrc("[00:20:72]hello"), vec![timed(20_720, "hello")]);
    }

    #[test]
    fn parses_no_fraction() {
        assert_eq!(parse_lrc("[01:05]x"), vec![timed(65_000, "x")]);
    }

    #[test]
    fn skips_metadata_tags() {
        let s =
            "[ti:Title]\n[ar:Artist]\n[al:Album]\n[by:User]\n[offset:300]\n[00:01.00]first line";
        assert_eq!(parse_lrc(s), vec![timed(1000, "first line")]);
    }

    #[test]
    fn expands_multi_timestamp_line() {
        assert_eq!(
            parse_lrc("[00:01.00][00:30.50]chorus"),
            vec![timed(1000, "chorus"), timed(30_500, "chorus")]
        );
    }

    // ───────────────── 新行为:JSON 行 / 无时间戳 / 混排 / Design X ─────────────────

    #[test]
    fn parses_json_credit_line_with_timestamp() {
        // 阵雨 v1 的 credits 行:t 为时间戳,拼接各 tx。
        let s = r#"{"t":625,"c":[{"tx":"编曲: "},{"tx":"夜晚做决定"}]}"#;
        assert_eq!(parse_lrc(s), vec![timed(625, "编曲: 夜晚做决定")]);
    }

    #[test]
    fn parses_json_credit_line_without_timestamp() {
        // Tattoo 的 credits 行:无 t → 无时间戳行。
        let s = r#"{"c":[{"tx":"作词: "},{"tx":"TOOKOO"}]}"#;
        assert_eq!(parse_lrc(s), vec![plain("作词: TOOKOO")]);
    }

    #[test]
    fn keeps_untimed_plain_lines() {
        // Tattoo 正文:纯文本,无时间戳,整段保留。
        assert_eq!(
            parse_lrc("Do you really care\n你是否真的在意"),
            vec![plain("Do you really care"), plain("你是否真的在意")]
        );
    }

    #[test]
    fn mixed_timed_and_untimed_preserves_order() {
        // 前段带戳 credit、中段无戳正文、末尾带戳 → 无戳行排序键继承前一时间戳,序不乱。
        let s = "[00:00.00]credit\n无戳正文\n[04:00.00]版权";
        assert_eq!(
            parse_lrc(s),
            vec![
                timed(0, "credit"),
                plain("无戳正文"),
                timed(240_000, "版权")
            ]
        );
    }

    #[test]
    fn design_x_keeps_unknown_bracket_verbatim() {
        // 非时间戳、非已知 meta 的 [..] 整行原样当文本(不剥离)。
        assert_eq!(
            parse_lrc("[Verse 1]let it go"),
            vec![plain("[Verse 1]let it go")]
        );
    }

    #[test]
    fn handles_empty_and_blank_lines() {
        assert!(parse_lrc("").is_empty());
        assert!(parse_lrc("\n\n  \n").is_empty());
    }

    // ───────────────── 严出 / 定位 ─────────────────

    #[test]
    fn to_lrc_string_strict_output() {
        let lines = parse_lrc("[0:20:7]a\n[03:11.337]b");
        assert_eq!(to_lrc_string(&lines), "[00:20.70]a\n[03:11.33]b");
    }

    #[test]
    fn to_lrc_string_skips_untimed() {
        // 无时间戳行不进 MPRIS 导出,只吐带戳行。
        let lines = parse_lrc("[00:01.00]timed\n无戳行\n[00:02.00]again");
        assert_eq!(to_lrc_string(&lines), "[00:01.00]timed\n[00:02.00]again");
    }

    #[test]
    fn to_lrc_string_empty() {
        assert_eq!(to_lrc_string(&[]), "");
    }

    #[test]
    fn current_line_basic() {
        let lines = vec![timed(1000, "a"), timed(2000, "b"), timed(3000, "c")];
        assert_eq!(current_line(&lines, 0), None);
        assert_eq!(current_line(&lines, 999), None);
        assert_eq!(current_line(&lines, 1000), Some(0));
        assert_eq!(current_line(&lines, 1500), Some(0));
        assert_eq!(current_line(&lines, 2000), Some(1));
        assert_eq!(current_line(&lines, 5000), Some(2));
    }

    #[test]
    fn current_line_skips_untimed() {
        // 无时间戳行不参与定位;混排里只在带戳行间跳。
        let lines = vec![timed(1000, "a"), plain("free"), timed(3000, "c")];
        assert_eq!(
            current_line(&lines, 2000),
            Some(0),
            "停在带戳的 a,跳过 free"
        );
        assert_eq!(current_line(&lines, 3000), Some(2));
    }

    #[test]
    fn current_line_all_untimed_is_none() {
        let lines = vec![plain("x"), plain("y")];
        assert_eq!(current_line(&lines, 10_000), None);
    }

    #[test]
    fn has_timed_and_has_words() {
        let word_line = LyricLine {
            time_ms: Some(1000),
            kind: LineKind::Words {
                dur_ms: 500,
                words: vec![Word {
                    start_ms: 1000,
                    dur_ms: 500,
                    text: "hi".to_owned(),
                }],
            },
        };
        assert!(!has_timed(&[plain("x")]));
        assert!(has_timed(&[timed(1, "x")]));
        assert!(!has_words(&[timed(1, "x")]));
        assert!(has_words(&[word_line]));
    }

    proptest::proptest! {
        /// 任意字符串喂解析器都不 panic(脏输入鲁棒性)。
        #[test]
        fn parse_never_panics(s in ".*") {
            let _ = parse_lrc(&s);
        }

        /// 严出幂等:只吐带戳行,故 once 恒为纯标准 LRC,再 parse→串相等(自洽)。
        #[test]
        fn strict_output_is_idempotent(s in ".*") {
            let once = to_lrc_string(&parse_lrc(&s));
            let twice = to_lrc_string(&parse_lrc(&once));
            proptest::prop_assert_eq!(once, twice);
        }
    }
}

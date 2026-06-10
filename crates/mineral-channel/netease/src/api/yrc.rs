//! 网易云 yrc 逐字歌词文本解析:私有格式 → 通用 [`LyricLine`]。
//!
//! 网易实际返回**两种**行格式,同一首歌可能混用:
//!
//! 1. **LRC 风格**:`[行起始_ms,行时长_ms](字起始_ms,字时长_ms,0)字符串(字起始_ms,字时长_ms,0)字符串...`
//!    - 字时间戳是**绝对毫秒**
//!    - 圆括号第三个数字通常恒为 0,语义不明,忽略
//!
//! 2. **JSON 风格**:`{"t":行起始_ms,"c":[{"tx":"字符串","tr":[相对偏移_ms,字时长_ms]},...]}`
//!    - `tr[0]` 是**相对行起始**的偏移,字绝对时间 = `t + tr[0]`
//!    - 字全部带 `tr` → 逐字行([`LineKind::Words`]);全不带 `tr`(作词 / 作曲等
//!      credits)→ 整行纯文本行([`LineKind::Plain`]),**保留**而非丢弃
//!
//! 解析失败的行整行跳过,不抛错。无时间戳行的排序键继承上一带时间戳行(保持相对位置)。

use mineral_model::{LineKind, LyricLine, Word};

/// 解析整段 yrc 文本为一条 [`LyricLine`] 序列(带时间戳行按时间排序,credits 等无时间戳行
/// 保持在其前一带戳行之后)。空文本返回空 vec。
pub(crate) fn parse_yrc(s: &str) -> Vec<LyricLine> {
    let mut keyed = Vec::<(u64, LyricLine)>::new();
    let mut last_time: u64 = 0;
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed = if trimmed.starts_with('{') {
            parse_json_line(trimmed)
        } else if trimmed.starts_with('[') {
            parse_lrc_style_line(trimmed)
        } else {
            None
        };
        if let Some(line) = parsed {
            let key = line.time_ms.unwrap_or(last_time);
            if let Some(t) = line.time_ms {
                last_time = t;
            }
            keyed.push((key, line));
        }
    }
    keyed.sort_by_key(|(k, _)| *k);
    keyed.into_iter().map(|(_, line)| line).collect()
}

/// JSON 风格:`{"t":12345,"c":[{"tx":"hello","tr":[0,300]},...]}`。
/// 字全带 `tr` → 逐字行;否则当 credits 纯文本行(拼接各 `tx`)。`c` 空 / 文本空白返回 `None`。
fn parse_json_line(line: &str) -> Option<LyricLine> {
    /// 一个字单元(`tx` 文本 + 可选 `tr = [相对偏移, 时长]`)。
    #[derive(serde::Deserialize)]
    struct WordDto {
        #[serde(default)]
        tx: String,

        #[serde(default)]
        tr: Option<Vec<u64>>,
    }

    /// 一行(行起始 `t` + 字单元序列 `c`)。
    #[derive(serde::Deserialize)]
    struct LineDto {
        #[serde(default)]
        t: Option<u64>,

        #[serde(default)]
        c: Vec<WordDto>,
    }

    let dto = serde_json::from_str::<LineDto>(line).ok()?;
    if dto.c.is_empty() {
        return None;
    }
    let start = dto.t.unwrap_or(0);

    // 字全带 tr → 逐字行。
    if dto.c.iter().all(|w| w.tr.is_some()) {
        let mut words = Vec::<Word>::with_capacity(dto.c.len());
        let mut total_dur: u64 = 0;
        for w in dto.c {
            let tr = w.tr?;
            let offset = *tr.first()?;
            let dur = *tr.get(1)?;
            words.push(Word {
                start_ms: start.saturating_add(offset),
                dur_ms: dur,
                text: w.tx,
            });
            total_dur = total_dur.max(offset.saturating_add(dur));
        }
        return Some(LyricLine {
            time_ms: Some(start),
            kind: LineKind::Words {
                dur_ms: total_dur,
                words,
            },
            translation: None,
            romanization: None,
        });
    }

    // 否则 credits 纯文本行:拼接各 tx,保留(不再丢)。
    let text = dto
        .c
        .iter()
        .map(|w| w.tx.as_str())
        .collect::<String>()
        .trim()
        .to_owned();
    if text.is_empty() {
        return None;
    }
    Some(LyricLine {
        time_ms: dto.t,
        kind: LineKind::Plain(text),
        translation: None,
        romanization: None,
    })
}

/// 解析单行 `[start,dur]文本(s,d)文本(s,d)...` 形式的 LRC-style yrc 行;格式不符返回 `None`。
fn parse_lrc_style_line(line: &str) -> Option<LyricLine> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let header = rest.get(..close)?;
    let (start_ms, dur_ms) = parse_two_ints(header)?;
    let mut tail = rest.get(close + 1..)?;

    let mut words = Vec::<Word>::new();
    while let Some(open) = tail.find('(') {
        let after_open = tail.get(open + 1..)?;
        let close = after_open.find(')')?;
        let inside = after_open.get(..close)?;
        let (cs, cd) = parse_first_two_ints(inside)?;
        let after_close = after_open.get(close + 1..)?;
        // 字段文本一直到下一个 `(` 或行尾。
        let text_end = after_close.find('(').unwrap_or(after_close.len());
        let text = after_close.get(..text_end)?.to_owned();
        words.push(Word {
            start_ms: cs,
            dur_ms: cd,
            text,
        });
        tail = after_close.get(text_end..)?;
    }

    if words.is_empty() {
        return None;
    }
    Some(LyricLine {
        time_ms: Some(start_ms),
        kind: LineKind::Words { dur_ms, words },
        translation: None,
        romanization: None,
    })
}

/// 解析 `"a,b"` 两个 u64,严格要求只有一个逗号。
fn parse_two_ints(s: &str) -> Option<(u64, u64)> {
    let (a, b) = s.split_once(',')?;
    let a: u64 = a.trim().parse().ok()?;
    let b: u64 = b.trim().parse().ok()?;
    Some((a, b))
}

/// 解析 `"a,b,..."` 的前两个 u64,后续字段(如恒为 0 的第三数)忽略。
fn parse_first_two_ints(s: &str) -> Option<(u64, u64)> {
    let mut it = s.split(',');
    let a: u64 = it.next()?.trim().parse().ok()?;
    let b: u64 = it.next()?.trim().parse().ok()?;
    Some((a, b))
}

#[cfg(test)]
mod tests {
    use super::parse_yrc;
    use mineral_model::{LineKind, LyricLine, Word};

    /// 一条逐字行(断言构造器)。
    fn word_line(start: u64, dur: u64, words: Vec<Word>) -> LyricLine {
        LyricLine {
            time_ms: Some(start),
            kind: LineKind::Words { dur_ms: dur, words },
            translation: None,
            romanization: None,
        }
    }

    /// 序列里第一条逐字行(跳过 credits 纯文本行)。
    fn first_word_line(v: &[LyricLine]) -> Option<&LyricLine> {
        v.iter().find(|l| !l.kind.words().is_empty())
    }

    #[test]
    fn parses_basic_line() {
        let s = "[1000,2000](1000,300,0)Hello (1300,200,0)world";
        assert_eq!(
            parse_yrc(s),
            vec![word_line(
                1000,
                2000,
                vec![
                    Word {
                        start_ms: 1000,
                        dur_ms: 300,
                        text: "Hello ".to_owned(),
                    },
                    Word {
                        start_ms: 1300,
                        dur_ms: 200,
                        text: "world".to_owned(),
                    },
                ],
            )]
        );
    }

    #[test]
    fn keeps_json_credits_as_plain() {
        // credits(只有 tx,无 tr)现在保留为纯文本行,不再跳过。
        let s = r#"{"t":0,"c":[{"tx":"作词:"}]}
{"t":0,"c":[{"tx":"作曲:"}]}
[2000,1500](2000,500,0)歌词"#;
        let v = parse_yrc(s);
        assert_eq!(v.len(), 3, "2 条 credits + 1 条逐字行");
        assert_eq!(v.first(), Some(&LyricLine::timed(0, "作词:")));
        assert!(
            first_word_line(&v).and_then(|l| l.time_ms) == Some(2000),
            "逐字行在 2000ms"
        );
    }

    #[test]
    fn empty_and_blank_input() {
        assert!(parse_yrc("").is_empty());
        assert!(parse_yrc("\n\n  \n").is_empty());
    }

    #[test]
    fn skips_unparseable_lines() {
        let s = "[bad header]not a yrc line\n[1000,500](1000,500,0)ok";
        let v = parse_yrc(s);
        assert_eq!(v.len(), 1);
        assert_eq!(v.first().and_then(|l| l.time_ms), Some(1000));
    }

    #[test]
    fn parses_json_form() {
        let s = r#"{"t":12000,"c":[{"tx":"Hello ","tr":[0,300]},{"tx":"world","tr":[300,200]}]}"#;
        assert_eq!(
            parse_yrc(s),
            vec![word_line(
                12000,
                500,
                vec![
                    Word {
                        start_ms: 12000,
                        dur_ms: 300,
                        text: "Hello ".to_owned(),
                    },
                    Word {
                        start_ms: 12300,
                        dur_ms: 200,
                        text: "world".to_owned(),
                    },
                ],
            )]
        );
    }

    #[test]
    fn keeps_credits_without_tr_as_plain() {
        // 纯 credits 行(只有 tx,无 tr)保留为 Plain;逐字行照常。
        let s = r#"{"t":0,"c":[{"tx":"作词:"},{"tx":"某某"}]}
{"t":12000,"c":[{"tx":"first ","tr":[0,300]},{"tx":"line","tr":[300,200]}]}"#;
        let v = parse_yrc(s);
        assert_eq!(v.len(), 2);
        assert_eq!(v.first(), Some(&LyricLine::timed(0, "作词:某某")));
        assert_eq!(first_word_line(&v).and_then(|l| l.time_ms), Some(12000));
    }

    #[test]
    fn handles_chinese_chars() {
        let s = "[0,3000](0,500,0)床(500,500,0)前(1000,500,0)明(1500,500,0)月(2000,1000,0)光";
        let v = parse_yrc(s);
        assert_eq!(v.len(), 1);
        assert_eq!(v.first().map(|l| l.kind.words().len()), Some(5));
        assert_eq!(
            v.first().and_then(|l| l.kind.words().first()),
            Some(&Word {
                start_ms: 0,
                dur_ms: 500,
                text: "床".to_owned(),
            })
        );
    }

    // ───────────────── 真实数据 ─────────────────
    // 用真打网易云存下来的原始 yrc 文本(`lyric_fixtures/*.yrc`)做断言。开头几行 credits
    // (`{"t":..,"c":[{"tx":"作词: "},..]}`,纯 tx 无 tr)现在保留为 Plain 行,故断言走
    // `first_word_line` 取首条逐字行。

    #[test]
    fn real_english_word_by_word() {
        // Mineral《The Last Word Is Rejoice》—— 项目名来源乐队,英文逐字。
        let v = parse_yrc(include_str!("lyric_fixtures/rejoice.yrc"));
        let first = first_word_line(&v);
        assert_eq!(first.and_then(|l| l.time_ms), Some(91340));
        let words = first.map(|l| l.kind.words()).unwrap_or_default();
        // 英文按词切,尾随空格保留(渲染直接拼)。
        assert_eq!(words.first().map(|w| w.text.as_str()), Some("How "));
        assert_eq!(words.first().map(|w| w.start_ms), Some(91340));
        assert_eq!(words.first().map(|w| w.dur_ms), Some(840));
        // "How will I drink from that stream" = 7 个词单元。
        assert_eq!(words.len(), 7);
        assert_eq!(words.last().map(|w| w.text.as_str()), Some("stream"));
    }

    #[test]
    fn real_japanese_word_by_word() {
        // ひとひら《The Sound of Summer Coming》—— 日语逐字,按单字切。
        let v = parse_yrc(include_str!("lyric_fixtures/hitohira.yrc"));
        let first = first_word_line(&v);
        assert_eq!(first.and_then(|l| l.time_ms), Some(68490));
        let words = first.map(|l| l.kind.words()).unwrap_or_default();
        assert_eq!(words.first().map(|w| w.text.as_str()), Some("迷"));
        assert_eq!(words.first().map(|w| w.dur_ms), Some(2540));
        assert_eq!(words.get(1).map(|w| w.text.as_str()), Some("子"));
        // 阿拉伯数字也作为独立字单元出现(「2人」的 "2")。
        assert!(words.iter().any(|w| w.text == "2"));
    }
}

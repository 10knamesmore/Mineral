//! 网易云 yrc 逐字歌词文本解析:私有格式 → 通用 [`WordLine`]。
//!
//! 网易实际返回**两种**行格式,同一首歌可能混用:
//!
//! 1. **LRC 风格**:`[行起始_ms,行时长_ms](字起始_ms,字时长_ms,0)字符串(字起始_ms,字时长_ms,0)字符串...`
//!    - 字时间戳是**绝对毫秒**
//!    - 圆括号第三个数字通常恒为 0,语义不明,忽略
//!
//! 2. **JSON 风格**:`{"t":行起始_ms,"c":[{"tx":"字符串","tr":[相对偏移_ms,字时长_ms]},...]}`
//!    - `tr[0]` 是**相对行起始**的偏移,字绝对时间 = `t + tr[0]`
//!    - 有 `tx` 但无 `tr` 的字段是元数据(作词/作曲),整行视作 credits
//!    - 没有任何 `tr` 字段的 JSON 行整行跳过(纯 credits)
//!
//! 解析失败的行整行跳过,不抛错。

use mineral_model::{Word, WordLine};

/// 解析整段 yrc 文本为按时间升序的 [`WordLine`] 列表。空文本返回空 vec。
pub(crate) fn parse_yrc(s: &str) -> Vec<WordLine> {
    let mut out = Vec::<WordLine>::new();
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
            out.push(line);
        }
    }
    out.sort_by_key(|l| l.start_ms);
    out
}

/// JSON 风格:`{"t":12345,"c":[{"tx":"hello","tr":[0,300]},...]}`。
/// 没有任何 `tr` 字段(纯 credits)返回 None。
fn parse_json_line(line: &str) -> Option<WordLine> {
    #[derive(serde::Deserialize)]
    struct WordDto {
        tx: String,

        #[serde(default)]
        tr: Option<Vec<u64>>,
    }
    #[derive(serde::Deserialize)]
    struct LineDto {
        #[serde(default)]
        t: u64,

        #[serde(default)]
        c: Vec<WordDto>,
    }
    let dto: LineDto = serde_json::from_str(line).ok()?;
    if dto.c.is_empty() {
        return None;
    }
    let mut words = Vec::<Word>::with_capacity(dto.c.len());
    let mut total_dur: u64 = 0;
    for w in dto.c {
        let tr = w.tr?;
        let offset = *tr.first()?;
        let dur = *tr.get(1)?;
        words.push(Word {
            start_ms: dto.t.saturating_add(offset),
            dur_ms: dur,
            text: w.tx,
        });
        total_dur = total_dur.max(offset.saturating_add(dur));
    }
    Some(WordLine {
        start_ms: dto.t,
        dur_ms: total_dur,
        words,
    })
}

/// 解析单行 `[start,dur]文本(s,d)文本(s,d)...` 形式的 LRC-style yrc 行;格式不符返回 None。
fn parse_lrc_style_line(line: &str) -> Option<WordLine> {
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
        let text = after_close.get(..text_end)?.to_string();
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
    Some(WordLine {
        start_ms,
        dur_ms,
        words,
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
    use mineral_model::{Word, WordLine};

    #[test]
    fn parses_basic_line() {
        let s = "[1000,2000](1000,300,0)Hello (1300,200,0)world";
        assert_eq!(
            parse_yrc(s),
            vec![WordLine {
                start_ms: 1000,
                dur_ms: 2000,
                words: vec![
                    Word {
                        start_ms: 1000,
                        dur_ms: 300,
                        text: "Hello ".to_string()
                    },
                    Word {
                        start_ms: 1300,
                        dur_ms: 200,
                        text: "world".to_string()
                    },
                ],
            }]
        );
    }

    #[test]
    fn skips_json_metadata_lines() {
        let s = r#"{"t":0,"c":[{"tx":"作词:"}]}
{"t":0,"c":[{"tx":"作曲:"}]}
[2000,1500](2000,500,0)歌词"#;
        let v = parse_yrc(s);
        assert_eq!(v.len(), 1);
        assert_eq!(v.first().map(|l| l.start_ms), Some(2000));
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
        assert_eq!(v.first().map(|l| l.start_ms), Some(1000));
    }

    #[test]
    fn parses_json_form() {
        let s = r#"{"t":12000,"c":[{"tx":"Hello ","tr":[0,300]},{"tx":"world","tr":[300,200]}]}"#;
        assert_eq!(
            parse_yrc(s),
            vec![WordLine {
                start_ms: 12000,
                dur_ms: 500,
                words: vec![
                    Word {
                        start_ms: 12000,
                        dur_ms: 300,
                        text: "Hello ".to_string()
                    },
                    Word {
                        start_ms: 12300,
                        dur_ms: 200,
                        text: "world".to_string()
                    },
                ],
            }]
        );
    }

    #[test]
    fn json_credits_without_tr_are_skipped() {
        // 纯 credits 行(只有 tx,无 tr)应当跳过,不出现在结果里。
        let s = r#"{"t":0,"c":[{"tx":"作词:"},{"tx":"某某"}]}
{"t":12000,"c":[{"tx":"first ","tr":[0,300]},{"tx":"line","tr":[300,200]}]}"#;
        let v = parse_yrc(s);
        assert_eq!(v.len(), 1);
        assert_eq!(v.first().map(|l| l.start_ms), Some(12000));
    }

    #[test]
    fn handles_chinese_chars() {
        let s = "[0,3000](0,500,0)床(500,500,0)前(1000,500,0)明(1500,500,0)月(2000,1000,0)光";
        let v = parse_yrc(s);
        assert_eq!(v.len(), 1);
        assert_eq!(v.first().map(|l| l.words.len()), Some(5));
        assert_eq!(
            v.first().and_then(|l| l.words.first()),
            Some(&Word {
                start_ms: 0,
                dur_ms: 500,
                text: "床".to_string()
            })
        );
    }
}

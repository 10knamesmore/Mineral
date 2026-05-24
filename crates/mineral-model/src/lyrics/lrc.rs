//! [`LrcLyric`] / [`WordLyric`] 的解析(宽进)、序列化(严出)与按时间定位。
//!
//! LRC 是跨 channel 的事实标准,所以解析能力放在 model 共享:各 channel 把拿到的
//! 原始 LRC 文本喂进 [`LrcLyric::parse`] 完成「反序列化 + 清洗」,得到结构化数据;
//! 要交给外部系统(如 MPRIS `xesam:asText`)时用 [`LrcLyric::to_lrc_string`] 重新
//! 序列化成标准格式。逐字歌词的私有文本格式由各 channel 自行解析,这里只提供 model
//! 侧通用的按时间定位 [`WordLyric::current_index`]。

use super::types::{LrcLine, LrcLyric, WordLyric};

impl LrcLyric {
    /// 解析一段 LRC 文本为「按时间升序」的行级歌词。
    ///
    /// 宽进:兼容时间戳分隔符 `.`(标准)与 `:`(网易把厘秒也用冒号分隔的变体)、无小数
    /// (整秒)、毫秒位 1~3、一行多个时间戳前缀(展开成多条);跳过 `[ti:]` 等元 tag 与
    /// 混入的逐字 JSON 行(`{` 开头)。空文本 / 解析失败的行直接跳过。
    ///
    /// # Params:
    ///   - `s`: 原始 LRC 文本。
    ///
    /// # Return:
    ///   按 `time_ms` 升序排好的行级歌词。
    pub fn parse(s: &str) -> Self {
        let mut out = Vec::<LrcLine>::new();
        for line in s.lines() {
            // 混入的逐字 JSON 行(`{...}`)不是 LRC,整行跳过。
            if line.trim_start().starts_with('{') {
                continue;
            }
            parse_line(line, &mut out);
        }
        out.sort_by_key(|l| l.time_ms);
        Self(out)
    }

    /// 序列化成标准 LRC 字符串(严出,厘秒精度 `[mm:ss.xx]text`)。
    ///
    /// 给外部系统(如 MPRIS `xesam:asText`)用;空歌词返回空串。
    ///
    /// # Return:
    ///   逐行 `[mm:ss.xx]text`、以 `\n` 连接的标准 LRC 文本。
    pub fn to_lrc_string(&self) -> String {
        self.iter()
            .map(|l| format_lrc_line(l.time_ms, &l.text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 二分找 `position_ms` 对应的当前行 index。
    ///
    /// 返回最后一个 `time_ms <= position_ms` 的行 index;若在第一行之前返回 None。
    pub fn current_index(&self, position_ms: u64) -> Option<usize> {
        if self.is_empty() || position_ms < self.first()?.time_ms {
            return None;
        }
        let pp = self.partition_point(|l| l.time_ms <= position_ms);
        if pp == 0 { None } else { Some(pp - 1) }
    }
}

impl WordLyric {
    /// 二分找 `position_ms` 对应的当前行 index。
    ///
    /// 返回最后一个 `start_ms <= position_ms` 的行 index;若在第一行之前返回 None。
    pub fn current_index(&self, position_ms: u64) -> Option<usize> {
        if self.is_empty() || position_ms < self.first()?.start_ms {
            return None;
        }
        let pp = self.partition_point(|l| l.start_ms <= position_ms);
        if pp == 0 { None } else { Some(pp - 1) }
    }
}

/// 把一行展开追加到 out。一行可能含多个时间戳前缀 + 一段共享文本。
fn parse_line(line: &str, out: &mut Vec<LrcLine>) {
    let mut rest = line;
    let mut stamps = Vec::<u64>::new();

    loop {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('[') {
            // 没有更多时间戳,trimmed 是文本
            if !stamps.is_empty() {
                let text = trimmed.trim().to_string();
                for ms in &stamps {
                    out.push(LrcLine {
                        time_ms: *ms,
                        text: text.clone(),
                    });
                }
            }
            return;
        }

        let Some(close) = trimmed.find(']') else {
            return;
        };
        let Some(inside) = trimmed.get(1..close) else {
            return;
        };
        let Some(next) = trimmed.get(close + 1..) else {
            return;
        };
        rest = next;

        if let Some(ms) = parse_timestamp(inside) {
            stamps.push(ms);
        }
        // 不是时间戳(metadata 或空)就忽略,继续找下一个
    }
}

/// 解析 `mm:ss.xx` / `mm:ss.xxx` / `mm:ss:xx`(冒号厘秒变体) / `mm:ss`(无小数)时间戳。
/// 元数据 tag 返回 None。
fn parse_timestamp(s: &str) -> Option<u64> {
    // 至少 `m:s` 长度 3,且第一个字符是数字。
    if !s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let colon = s.find(':')?;
    let mm: u64 = s.get(..colon)?.parse().ok()?;
    let after = s.get(colon + 1..)?;

    // 分隔符兼容 `.`(标准)与 `:`(网易厘秒冒号变体,如 [mm:ss:xx]);
    // 无分隔符则视为整秒(如 [mm:ss])。
    let (sec_part, ms_part) = match after.find(['.', ':']) {
        Some(sep) => (after.get(..sep)?, after.get(sep + 1..)?),
        None => (after, ""),
    };
    let ss: u64 = sec_part.parse().ok()?;
    let ms: u64 = if ms_part.is_empty() {
        0
    } else {
        // 1 位 = 百毫秒,2 位 = 十毫秒,3 位 = 毫秒。补足后再换算。
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
    use super::super::types::{LrcLine, Word, WordLine};
    use super::{LrcLyric, WordLyric};

    fn lrc(time_ms: u64, text: &str) -> LrcLine {
        LrcLine {
            time_ms,
            text: text.to_string(),
        }
    }

    #[test]
    fn parses_basic_lrc() {
        let s = "[00:01.00]hello\n[00:02.50]world\n[01:00.123]end";
        assert_eq!(
            LrcLyric::parse(s).to_vec(),
            vec![lrc(1000, "hello"), lrc(2500, "world"), lrc(60_123, "end")]
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
        insta::with_settings!({ description => "真实 LRC:metadata 跳过、多时间戳展开、CJK、厘秒" }, {
            insta::assert_debug_snapshot!(LrcLyric::parse(s).to_vec());
        });
    }

    #[test]
    fn parses_colon_centisecond_variant() {
        // 网易把厘秒也用冒号分隔的变体 [mm:ss:xx],应与点格式等价。
        assert_eq!(
            LrcLyric::parse("[00:20:72]hello").to_vec(),
            vec![lrc(20_720, "hello")]
        );
    }

    #[test]
    fn parses_no_fraction() {
        assert_eq!(LrcLyric::parse("[01:05]x").to_vec(), vec![lrc(65_000, "x")]);
    }

    #[test]
    fn skips_metadata_tags() {
        let s =
            "[ti:Title]\n[ar:Artist]\n[al:Album]\n[by:User]\n[offset:300]\n[00:01.00]first line";
        assert_eq!(LrcLyric::parse(s).to_vec(), vec![lrc(1000, "first line")]);
    }

    #[test]
    fn handles_empty_and_blank_lines() {
        assert!(LrcLyric::parse("").is_empty());
        assert!(LrcLyric::parse("\n\n  \n").is_empty());
    }

    #[test]
    fn expands_multi_timestamp_line() {
        assert_eq!(
            LrcLyric::parse("[00:01.00][00:30.50]chorus").to_vec(),
            vec![lrc(1000, "chorus"), lrc(30_500, "chorus")]
        );
    }

    #[test]
    fn filters_word_json_lines() {
        // 混入的逐字 JSON 行(`{` 开头)整行过滤,只留真正的 LRC。
        let raw = "{\"t\":0,\"c\":[]}\n[00:01.00]真正的歌词";
        assert_eq!(LrcLyric::parse(raw).to_vec(), vec![lrc(1000, "真正的歌词")]);
    }

    #[test]
    fn to_lrc_string_strict_output() {
        // 宽进的各种变体 → 严出统一成标准 [mm:ss.xx]。
        let lyric = LrcLyric::parse("[0:20:7]a\n[03:11.337]b");
        assert_eq!(lyric.to_lrc_string(), "[00:20.70]a\n[03:11.33]b");
    }

    #[test]
    fn to_lrc_string_empty() {
        assert_eq!(LrcLyric::default().to_lrc_string(), "");
    }

    #[test]
    fn current_index_lrc_basic() {
        let lyric = LrcLyric(vec![lrc(1000, "a"), lrc(2000, "b"), lrc(3000, "c")]);
        assert_eq!(lyric.current_index(0), None);
        assert_eq!(lyric.current_index(999), None);
        assert_eq!(lyric.current_index(1000), Some(0));
        assert_eq!(lyric.current_index(1500), Some(0));
        assert_eq!(lyric.current_index(2000), Some(1));
        assert_eq!(lyric.current_index(5000), Some(2));
    }

    #[test]
    fn current_index_words_basic() {
        let line = |start_ms: u64| WordLine {
            start_ms,
            dur_ms: 500,
            words: Vec::<Word>::new(),
        };
        let lyric = WordLyric(vec![line(1000), line(2000), line(3000)]);
        assert_eq!(lyric.current_index(0), None);
        assert_eq!(lyric.current_index(999), None);
        assert_eq!(lyric.current_index(1000), Some(0));
        assert_eq!(lyric.current_index(1500), Some(0));
        assert_eq!(lyric.current_index(2000), Some(1));
        assert_eq!(lyric.current_index(5000), Some(2));
    }
}

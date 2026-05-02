//! 极简 LRC 解析:`[mm:ss.xx]text` → `(ms, text)`。
//!
//! 兼容:
//! - 一行多个时间戳(`[00:01.00][00:30.50]同一段歌词`)展开成两条
//! - 元数据 tag(`[ti:..]` / `[ar:..]` / `[al:..]` / `[by:..]` / `[offset:..]`)跳过
//! - 毫秒位 1~3 位均可(`.xx` / `.xxx`)
//!
//! 不做:
//! - yrc 逐字
//! - `[offset:..]` 实际加到时间戳上(本期歌词够准了不补偿)

/// 解析一段 LRC 文本为「按时间升序」的 `(timestamp_ms, text)` 列表。
///
/// 空文本返回空 vec;解析失败的行直接跳过。
pub fn parse_lrc(s: &str) -> Vec<(u64, String)> {
    let mut out = Vec::<(u64, String)>::new();
    for line in s.lines() {
        parse_line(line, &mut out);
    }
    out.sort_by_key(|(ms, _)| *ms);
    out
}

/// 把一行展开追加到 out。一行可能含多个时间戳前缀 + 一段共享文本。
fn parse_line(line: &str, out: &mut Vec<(u64, String)>) {
    let mut rest = line;
    let mut stamps = Vec::<u64>::new();

    loop {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('[') {
            // 没有更多时间戳,trimmed 是文本
            if !stamps.is_empty() {
                let text = trimmed.trim().to_string();
                for ms in &stamps {
                    out.push((*ms, text.clone()));
                }
            }
            return;
        }

        let Some(close) = trimmed.find(']') else {
            return;
        };
        let inside = &trimmed[1..close];
        rest = &trimmed[close + 1..];

        if let Some(ms) = parse_timestamp(inside) {
            stamps.push(ms);
        }
        // 不是时间戳(metadata 或空)就忽略,继续找下一个
    }
}

/// 解析 `mm:ss.xx` / `mm:ss.xxx` / `mm:ss` 时间戳。元数据 tag 返回 None。
fn parse_timestamp(s: &str) -> Option<u64> {
    // 至少 `m:s` 长度 3,且第一个字符是数字。
    if !s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let colon = s.find(':')?;
    let mm: u64 = s.get(..colon)?.parse().ok()?;
    let after = s.get(colon + 1..)?;

    let (sec_part, ms_part) = match after.find('.') {
        Some(dot) => (after.get(..dot)?, after.get(dot + 1..)?),
        None => (after, ""),
    };
    let ss: u64 = sec_part.parse().ok()?;
    let ms: u64 = if ms_part.is_empty() {
        0
    } else {
        // 1 位 = 百毫秒,2 位 = 十毫秒,3 位 = 毫秒。补足 3 位 *再除*。
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

/// 在按时间升序的 `lines` 里二分找 `position_ms` 对应的当前行 index。
///
/// 返回最后一个 `lines[i].0 <= position_ms` 的 i;若 `position_ms` 在第一行之前返回 None。
pub fn current_index(lines: &[(u64, String)], position_ms: u64) -> Option<usize> {
    if lines.is_empty() || position_ms < lines.first()?.0 {
        return None;
    }
    // 等价 partition_point(|x| x.0 <= position_ms) - 1
    let pp = lines.partition_point(|(t, _)| *t <= position_ms);
    if pp == 0 {
        None
    } else {
        Some(pp - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_lrc() {
        let s = "[00:01.00]hello\n[00:02.50]world\n[01:00.123]end";
        let v = parse_lrc(s);
        assert_eq!(
            v,
            vec![
                (1000, "hello".to_string()),
                (2500, "world".to_string()),
                (60_123, "end".to_string()),
            ]
        );
    }

    #[test]
    fn skips_metadata_tags() {
        let s =
            "[ti:Title]\n[ar:Artist]\n[al:Album]\n[by:User]\n[offset:300]\n[00:01.00]first line";
        let v = parse_lrc(s);
        assert_eq!(v, vec![(1000, "first line".to_string())]);
    }

    #[test]
    fn handles_empty_and_blank_lines() {
        assert!(parse_lrc("").is_empty());
        assert!(parse_lrc("\n\n  \n").is_empty());
    }

    #[test]
    fn expands_multi_timestamp_line() {
        let v = parse_lrc("[00:01.00][00:30.50]chorus");
        assert_eq!(
            v,
            vec![(1000, "chorus".to_string()), (30_500, "chorus".to_string()),]
        );
    }

    #[test]
    fn current_index_basic() {
        let lines = vec![
            (1000, "a".to_string()),
            (2000, "b".to_string()),
            (3000, "c".to_string()),
        ];
        assert_eq!(current_index(&lines, 0), None);
        assert_eq!(current_index(&lines, 999), None);
        assert_eq!(current_index(&lines, 1000), Some(0));
        assert_eq!(current_index(&lines, 1500), Some(0));
        assert_eq!(current_index(&lines, 2000), Some(1));
        assert_eq!(current_index(&lines, 5000), Some(2));
    }
}

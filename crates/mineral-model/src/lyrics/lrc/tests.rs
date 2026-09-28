//! Verifies LRC parsing, ordering, word timing and text export at the public model boundary.

use super::{current_line, has_timed, has_words, parse_lrc, to_lrc_string};
use crate::lyrics::{LineKind, LyricLine, Word};

/// 带时间戳文本行(断言构造器)。
fn timed(time_ms: u64, text: &str) -> LyricLine {
    LyricLine::timed(time_ms, text)
}

/// 无时间戳文本行(断言构造器)。
fn plain(text: &str) -> LyricLine {
    LyricLine::untimed(text)
}

/// 带已知字时长的行(断言构造器)。
fn words(time_ms: u64, dur_ms: u64, parts: &[(u64, u64, &str)]) -> LyricLine {
    LyricLine {
        time_ms: Some(time_ms),
        kind: LineKind::Words {
            dur_ms,
            words: parts
                .iter()
                .map(|&(start_ms, dur_ms, text)| Word {
                    start_ms,
                    dur_ms,
                    text: text.to_owned(),
                })
                .collect(),
        },
        translation: None,
        romanization: None,
    }
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
    let s = "[ti:Title]\n[ar:Artist]\n[al:Album]\n[by:User]\n[offset:0]\n[00:01.00]first line";
    assert_eq!(parse_lrc(s), vec![timed(1000, "first line")]);
}

#[test]
fn expands_multi_timestamp_line() {
    assert_eq!(
        parse_lrc("[00:01.00][00:30.50]chorus"),
        vec![timed(1000, "chorus"), timed(30_500, "chorus")]
    );
}

#[test]
fn ignores_document_bom_for_lrc_json_and_plain_text() {
    for input in [
        "[00:01.00]hello",
        r#"{"t":625,"c":[{"tx":"credit"}]}"#,
        "untimed text",
    ] {
        assert_eq!(parse_lrc(&format!("\u{feff}{input}")), parse_lrc(input));
    }
}

#[test]
fn applies_global_offset_with_positive_values_earlier() {
    assert_eq!(
        parse_lrc("[00:01.00]first\n[00:02.00]second\n[offset:+300]"),
        vec![timed(700, "first"), timed(1_700, "second")]
    );
    assert_eq!(
        parse_lrc("[offset:-300]\n[00:01.00]first"),
        vec![timed(1_300, "first")]
    );
    assert_eq!(
        parse_lrc("[offset:+1500]\n[00:01.00]first\n[00:02.00]second"),
        vec![timed(0, "first"), timed(500, "second")]
    );
}

#[test]
fn last_valid_offset_wins_without_changing_json_times() {
    let input = "[offset:100]\n[offset:-200]\n[offset:bad]\n\
                     [offset:18446744073709551616]\n\
                     {\"t\":500,\"c\":[{\"tx\":\"credit\"}]}\n\
                     [00:01]lyric\nuntimed";
    assert_eq!(
        parse_lrc(input),
        vec![
            timed(500, "credit"),
            timed(1_200, "lyric"),
            plain("untimed")
        ]
    );
}

#[test]
fn preserves_invalid_and_overflowing_timestamps_as_text() {
    for timestamp in [
        "00:60.00",
        "00:+1.00",
        "00:001.00",
        "00:01.",
        "00:01.12x",
        "00:01.1234",
        "00:01.123-1x",
        "18446744073709551615:00",
        "18446744073709551616:00",
        "307445734561825:51.616",
    ] {
        let input = format!("[{timestamp}]text");
        assert_eq!(parse_lrc(&input), vec![plain(&input)]);
    }
    assert_eq!(
        parse_lrc("[307445734561825:51.615]edge"),
        vec![timed(u64::MAX, "edge")]
    );
}

#[test]
fn handles_offset_arithmetic_at_integer_limits() {
    let input = "[offset:-9223372036854775808]\n[00:00]first\n\
                     [307445734561825:51.615]outside";
    assert_eq!(
        parse_lrc(input),
        vec![timed(i64::MIN.unsigned_abs(), "first"), plain("outside")]
    );
    assert_eq!(
        parse_lrc("[offset:+9223372036854775807]\n[00:01]first"),
        vec![timed(0, "first")]
    );
}

#[test]
fn parses_enhanced_words_with_explicit_terminal_tag() {
    assert_eq!(
        parse_lrc("[00:01]<00:01.20>Hello <00:01.75>world<00:02.50>"),
        vec![words(
            1_000,
            1_500,
            &[(1_200, 550, "Hello "), (1_750, 750, "world")]
        )]
    );
    assert_eq!(
        parse_lrc("[00:01]<00:01>你<00:01.25>好<00:02>"),
        vec![words(
            1_000,
            1_000,
            &[(1_000, 250, "你"), (1_250, 750, "好")]
        )]
    );
}

#[test]
fn infers_enhanced_last_word_end_from_next_distinct_line_time() {
    let input = "[00:01]<00:01>Hello <00:01.50>world\nuntimed\n\
                     [00:03]next\n[00:01]same start";
    assert_eq!(
        parse_lrc(input),
        vec![
            words(
                1_000,
                2_000,
                &[(1_000, 500, "Hello "), (1_500, 1_500, "world")]
            ),
            plain("untimed"),
            timed(1_000, "same start"),
            timed(3_000, "next"),
        ]
    );
}

#[test]
fn enhanced_empty_tags_bound_gaps_and_empty_lines_bound_last_words() {
    assert_eq!(
        parse_lrc("[00:01]<00:01>A<00:01.50><00:02>B<00:03>"),
        vec![words(
            1_000,
            2_000,
            &[(1_000, 500, "A"), (2_000, 1_000, "B")]
        )]
    );
    assert_eq!(
        parse_lrc("[00:01]<00:01>A\n[00:02]"),
        vec![
            words(1_000, 1_000, &[(1_000, 1_000, "A")]),
            timed(2_000, "")
        ]
    );
}

#[test]
fn applies_offset_to_enhanced_boundaries_before_computing_durations() {
    assert_eq!(
        parse_lrc("[offset:+1500]\n[00:01]<00:01>A<00:02>B<00:03>"),
        vec![words(0, 1_500, &[(0, 500, "A"), (500, 1_000, "B")])]
    );
    assert_eq!(
        parse_lrc("[offset:-200]\n[00:01]<00:01>A\n[00:02]"),
        vec![
            words(1_200, 1_000, &[(1_200, 1_000, "A")]),
            timed(2_200, "")
        ]
    );
}

#[test]
fn keeps_enhanced_text_when_word_ends_are_unknown_or_inconsistent() {
    for input in [
        "[00:01]<00:01>Hello <00:01.50>world",
        "[00:01]<00:02>Hello <00:01.50>world<00:03>",
        "[00:01]<00:00.50>Hello <00:01.50>world<00:03>",
    ] {
        assert_eq!(parse_lrc(input), vec![timed(1_000, "Hello world")]);
    }
    assert_eq!(
        parse_lrc("[00:01]<00:01>Hello <00:02>world\n[00:01.50]next"),
        vec![timed(1_000, "Hello world"), timed(1_500, "next")]
    );
}

#[test]
fn preserves_enhanced_text_when_adjusted_word_time_overflows() {
    let input = "[offset:-1]\n[00:01]<00:01>A<307445734561825:51.615>";
    assert_eq!(parse_lrc(input), vec![timed(1_001, "A")]);
}

#[test]
fn preserves_literal_brackets_and_plain_text_without_line_time() {
    assert_eq!(
        parse_lrc(
            "<00:01>untimed\n[00:01]literal <tag>\n\
                       [00:02]<00:02>A <tag><00:03>B<00:04>"
        ),
        vec![
            plain("<00:01>untimed"),
            timed(1_000, "literal <tag>"),
            words(
                2_000,
                2_000,
                &[(2_000, 1_000, "A <tag>"), (3_000, 1_000, "B")]
            ),
        ]
    );
    assert_eq!(
        parse_lrc("[00:01]<18446744073709551615:00>text"),
        vec![timed(1_000, "<18446744073709551615:00>text")]
    );
}

#[test]
fn repeated_enhanced_lines_keep_text_without_guessing_word_replays() {
    assert_eq!(
        parse_lrc("[00:01][00:03]<00:01>A<00:01.50>B<00:02>"),
        vec![timed(1_000, "AB"), timed(3_000, "AB")]
    );
}

/// 同时刻的逐字行共享下一边界,无时间文本保持原有相对顺序。
#[test]
fn equal_time_enhanced_lines_share_the_next_boundary() {
    assert_eq!(
        parse_lrc("[00:01]<00:01>A\nuntimed\n[00:01]<00:01.50>B\n[00:03]next"),
        vec![
            words(1_000, 2_000, &[(1_000, 2_000, "A")]),
            plain("untimed"),
            words(1_000, 2_000, &[(1_500, 1_500, "B")]),
            timed(3_000, "next"),
        ]
    );
}

/// 相同字时间允许零时长,显式终点不被下一行裁剪。
#[test]
fn preserves_zero_duration_words_and_explicit_overlapping_ends() {
    assert_eq!(
        parse_lrc("[00:01]<00:01>A<00:01>B<00:03>\n[00:02]next"),
        vec![
            words(1_000, 2_000, &[(1_000, 0, "A"), (1_000, 2_000, "B")]),
            timed(2_000, "next"),
        ]
    );
}

/// UTF-8 前缀按行时间起唱,只裁整行外部空白,保留内部空格与普通尖括号。
#[test]
fn preserves_unicode_prefixes_and_internal_word_spacing() {
    assert_eq!(
        parse_lrc("[00:01]  引子 <00:02>你🎵 <tag><00:03>好   <00:04>  "),
        vec![words(
            1_000,
            3_000,
            &[
                (1_000, 1_000, "引子 "),
                (2_000, 1_000, "你🎵 <tag>"),
                (3_000, 1_000, "好"),
            ],
        )]
    );
}

// ───────────────── JSON 行 / 无时间戳 / 混排 ─────────────────

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
fn preserves_braced_plain_text_but_skips_empty_json_credits() {
    assert_eq!(
        parse_lrc("{chorus}\n{一起唱}\n{\"c\":[]}"),
        vec![plain("{chorus}"), plain("{一起唱}")]
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
fn unknown_bracket_line_is_preserved_verbatim() {
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

#[test]
fn negative_t_json_line_is_untimed() {
    // 网易给纯器乐歌(如 Charmer《Split Plate》)的 credits 行 t=-1(无时间轴哨兵),
    // 应解析成无时间戳行,不能因 u64 反序列化失败把整行静默丢掉。
    let s = r#"{"t":-1,"c":[{"tx":"作词: "},{"tx":"Someone"}]}"#;
    assert_eq!(parse_lrc(s), vec![plain("作词: Someone")]);
}

#[test]
fn malformed_negative_timestamp_is_stripped() {
    // 同一首歌走老接口时,服务端把 t=-1 格式化成畸形 `[00:00.00-1]`(负毫秒拼进
    // 厘秒位)。应剥掉括号当无时间戳行,不能把畸形时间戳当正文渲染出去。
    let s = "[00:00.00-1] 作词 : Someone\n[00:00.00-1] 作曲 : Someone";
    assert_eq!(
        parse_lrc(s),
        vec![plain("作词 : Someone"), plain("作曲 : Someone")]
    );
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
        translation: None,
        romanization: None,
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

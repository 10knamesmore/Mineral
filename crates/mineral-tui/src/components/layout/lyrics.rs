//! Lyrics 面板:按 [`crate::runtime::state::AppState::current_lyrics`] 渲染当前行 + 邻近行,
//! 当前行高亮居中,上下各若干行 dim。无歌词时 fallback "♪ no lyrics"。
//!
//! 有逐字歌词时,中心行走字级 wipe 渲染:已唱的字 = `theme.text` + Bold,
//! 未唱的字 = `theme.overlay` dim。邻行无论是否有逐字都按整行 dim 渲染。
//!
//! `t` 键打开副歌词(翻译 / 罗马音)后,每个可见原文行下方紧跟一条静态副行;
//! 副行不参与 wipe,恒按 muted 样式渲染。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use mineral_model::{LrcLyric, WordLine, WordLyric};

use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, LyricExtra};

/// 渲染 lyrics 面板到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let word_lines = state.current_words().filter(|v| !v.is_empty());
    let lrc_lines = state.current_lyrics().filter(|v| !v.is_empty());
    let extra = state.current_extra_lyric();

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(title_left_spans(
            word_lines.is_some(),
            lrc_lines.is_some(),
            theme,
        )))
        .title_top(
            Line::from(title_right_spans(
                state.has_extra_lyrics(),
                extra.map(|_| state.lyric_extra),
                theme,
            ))
            .right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let position_ms = state.playback.position_ms;
    // 有逐字时整个窗口都从逐字推:中心行 + 上下行同源,index 唯一,wipe 不会跟邻行错位。
    // 没逐字时回落到 lrc 行级整行高亮(原始路径)。
    let (lines, cur, cursor) = if let Some(wl) = word_lines {
        let lines: Vec<(u64, String)> = wl
            .iter()
            .map(|l| {
                let text: String = l.words.iter().map(|w| w.text.as_str()).collect();
                (l.start_ms, text)
            })
            .collect();
        let cur = wl.current_index(position_ms);
        let cursor = WordCursor {
            lines: Some(wl),
            position_ms,
        };
        (lines, cur, cursor)
    } else {
        let Some(lrc) = lrc_lines else {
            draw_fallback(frame, inner, theme);
            return;
        };
        let cur = lrc.current_index(position_ms);
        let lines: Vec<(u64, String)> = lrc.iter().map(|l| (l.time_ms, l.text.clone())).collect();
        let cursor = WordCursor {
            lines: None,
            position_ms,
        };
        (lines, cur, cursor)
    };
    paint_window(frame, inner, &lines, cur, cursor, extra, theme);
}

/// 左上标识:数据档(`lyrics` / `synced` / `synced ✦`)。两档同步用不同高亮区分——
/// 行级 `synced` 用 accent_2(sapphire),逐字 `synced ✦` 用 accent(mauve);`lyrics · `
/// 前缀恒 subtext 弱化。
///
/// # Params:
///   - `has_words`: 是否有逐字歌词
///   - `has_lrc`: 是否有行级 LRC
///   - `theme`: 取色
///
/// # Return:
///   组成 ` lyrics · synced ✦ ` 的分色 Span 序列(首尾留空格)。
fn title_left_spans(has_words: bool, has_lrc: bool, theme: &Theme) -> Vec<Span<'static>> {
    let base = Style::new().fg(theme.subtext);
    let base_lyrics = Span::styled(" lyrics · ", base);

    if has_words {
        vec![
            base_lyrics,
            Span::styled(
                "synced ✦",
                Style::new()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::ITALIC),
            ),
            Span::styled(" ", base),
        ]
    } else if has_lrc {
        vec![
            base_lyrics,
            Span::styled(
                "synced",
                Style::new()
                    .fg(theme.accent_2)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::ITALIC),
            ),
            Span::styled(" ", base),
        ]
    } else {
        vec![Span::styled(" lyrics ", base)]
    }
}

/// 右上提示:当前生效的副歌词档 + `[t]` 按键提示(方括号示意这是个按键)。翻译标 `tr`
/// (green)、罗马音标 `ro`(peach),均 bold + italic;`[t]` 及分隔点用 overlay 弱化。
/// 没有任何副歌词可切换时返回空序列(不显示提示)。
///
/// # Params:
///   - `has_extra`: 是否有任一副歌词(翻译 / 罗马音)可切换
///   - `extra`: 当前生效(且非空)的副歌词档;`None` 只显示按键
///   - `theme`: 取色
///
/// # Return:
///   组成 ` tr · [t] ` / ` [t] ` 的分色 Span 序列;无副歌词时为空。
fn title_right_spans(
    has_extra: bool,
    extra: Option<LyricExtra>,
    theme: &Theme,
) -> Vec<Span<'static>> {
    if !has_extra {
        return Vec::new();
    }
    let key = Style::new().fg(theme.overlay);
    let mark = |color| {
        Style::new()
            .fg(color)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::ITALIC)
    };
    let mut spans = vec![Span::styled(" ", key)];
    match extra {
        Some(LyricExtra::Translation) => {
            spans.push(Span::styled("tr", mark(theme.green)));
            spans.push(Span::styled(" · ", key));
        }
        Some(LyricExtra::Romanization) => {
            spans.push(Span::styled("ro", mark(theme.peach)));
            spans.push(Span::styled(" · ", key));
        }
        Some(LyricExtra::None) | None => {}
    }
    spans.push(Span::styled("[t]", key));
    spans.push(Span::styled(" ", key));
    spans
}

/// 渲染时传入的逐字上下文(打包以减少 paint_window 参数数)。
#[derive(Clone, Copy)]
struct WordCursor<'a> {
    /// 逐字歌词行;`None` 表示没逐字(走 lrc 整行高亮)。
    lines: Option<&'a WordLyric>,

    /// 当前播放位置(ms),用于 wipe 进度计算。
    position_ms: u64,
}

/// 一个视觉行:对应一个原文行(`Primary`)或其下方的副歌词(`Secondary`)。
enum Cell {
    /// 原文行,引用 `lines` 中的索引。
    Primary {
        /// 在 `lines` 中的行索引。
        line_idx: usize,
    },

    /// 副歌词行(翻译 / 罗马音),自带文本。
    Secondary {
        /// 副歌词文本。
        text: String,
    },
}

/// 没歌词时居中渲染一行 `♪ no lyrics`(灰色 + 斜体)。
fn draw_fallback(frame: &mut Frame<'_>, inner: Rect, theme: &Theme) {
    let centered_y = inner.y + inner.height / 2;
    let text_area = Rect::new(inner.x, centered_y, inner.width, 1);
    let line = Line::from("♪ no lyrics").style(
        Style::new()
            .fg(theme.overlay)
            .add_modifier(Modifier::ITALIC),
    );
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), text_area);
}

/// 按时间对齐取某原文行对应的副歌词文本。翻译 / 罗马音逐行时间戳与原文有 ~0.1s 量级
/// 抖动且**方向不定**(有时早、有时晚),故取与原文行时间**最接近**的一条,而非
/// `current_index` 的「≤ t 的最后一条」(后者在副歌词时间戳偏晚时会错配到上一句)。
/// 行数 / 索引也不保证与原文一致(原文有空白间奏行),所以不能按索引硬配对。
fn secondary_text(extra: Option<&LrcLyric>, time_ms: u64) -> Option<String> {
    let e = extra?;
    let i = nearest_index(e, time_ms)?;
    e.get(i).map(|l| l.text.clone()).filter(|t| !t.is_empty())
}

/// 找时间戳与 `t` 最接近的行 index(候选只可能是 `≤ t` 的最后一条与 `> t` 的第一条)。
fn nearest_index(lines: &LrcLyric, t: u64) -> Option<usize> {
    if lines.is_empty() {
        return None;
    }
    let pp = lines.partition_point(|l| l.time_ms <= t);
    let before = pp.checked_sub(1);
    let after = (pp < lines.len()).then_some(pp);
    let dist = |i: usize| -> u64 {
        lines
            .get(i)
            .map_or(u64::MAX, |l| l.time_ms.max(t) - l.time_ms.min(t))
    };
    match (before, after) {
        (Some(b), Some(a)) => Some(if dist(b) <= dist(a) { b } else { a }),
        (Some(b), None) => Some(b),
        (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

/// 渲染以 `cur` 为中心、上下展开的歌词窗口。
///
/// 每个原文行展开成一个 `Primary` 视觉行;`extra` 非空时其下紧跟一个 `Secondary` 视觉行。
/// 居中基准是当前原文行的 `Primary`;中心行若有逐字走字级 wipe,否则整行高亮;
/// 其余 `Primary` 按距中心远近 dim,`Secondary` 恒 muted + 斜体。
fn paint_window(
    frame: &mut Frame<'_>,
    inner: Rect,
    lines: &[(u64, String)],
    cur: Option<usize>,
    cursor: WordCursor<'_>,
    extra: Option<&LrcLyric>,
    theme: &Theme,
) {
    // 展开成视觉行序列,并记下当前原文行所在的视觉行(居中基准)。
    let mut cells = Vec::<Cell>::new();
    let mut center_cell = 0usize;
    for (idx, (time_ms, primary)) in lines.iter().enumerate() {
        if Some(idx) == cur {
            center_cell = cells.len();
        }
        cells.push(Cell::Primary { line_idx: idx });
        // 空白间奏行(原文无字)不配副歌词。
        if !primary.is_empty()
            && let Some(text) = secondary_text(extra, *time_ms)
        {
            cells.push(Cell::Secondary { text });
        }
    }

    let height = usize::from(inner.height);
    let center_row = height / 2;
    // fade 用:从中心向两侧最远距离的较大者(窗口可能不对称)。
    let max_dist = u64::try_from(center_row.max(height.saturating_sub(center_row + 1)))
        .unwrap_or(0)
        .max(1);
    let denom = max_dist.saturating_sub(1).max(1);

    for row in 0..height {
        // 把行号映射到 cells 的 index:row=center_row 对应 center_cell。
        let cell_idx_signed = isize::try_from(row).unwrap_or(0)
            - isize::try_from(center_row).unwrap_or(0)
            + isize::try_from(center_cell).unwrap_or(0);
        if cell_idx_signed < 0 {
            continue;
        }
        let Ok(cell_idx) = usize::try_from(cell_idx_signed) else {
            continue;
        };
        let Some(cell) = cells.get(cell_idx) else {
            continue;
        };

        let row_u16 = u16::try_from(row).unwrap_or(0);
        let row_area = Rect::new(inner.x, inner.y + row_u16, inner.width, 1);

        let dist_signed =
            isize::try_from(row).unwrap_or(0) - isize::try_from(center_row).unwrap_or(0);
        let dist = u64::try_from(dist_signed.unsigned_abs()).unwrap_or(0);

        let line = render_cell(cell, lines, cur, cursor, dist, denom, theme);
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), row_area);
    }
}

/// 把一个视觉行渲成 [`Line`]:中心原文行高亮 / wipe,邻近原文行 dim,副歌词行恒 muted。
fn render_cell<'a>(
    cell: &'a Cell,
    lines: &'a [(u64, String)],
    cur: Option<usize>,
    cursor: WordCursor<'a>,
    dist: u64,
    denom: u64,
    theme: &Theme,
) -> Line<'a> {
    match cell {
        Cell::Secondary { text } => {
            // 副行永远比原文淡:从 overlay 起 fade 入背景,加斜体作视觉区分。
            let color = lerp_color(theme.overlay, theme.surface0, dist.saturating_sub(1), denom);
            Line::from(text.as_str()).style(Style::new().fg(color).add_modifier(Modifier::ITALIC))
        }
        Cell::Primary { line_idx } => {
            let text = lines.get(*line_idx).map_or("", |(_, t)| t.as_str());
            if Some(*line_idx) == cur {
                // 中心行用 accent(mauve)区别于灰阶 fade,色相差让聚焦行一眼可辨;
                // 有逐字则走字级 wipe(同一 line_idx 取逐字行,与 lines 1:1)。
                cursor.lines.and_then(|v| v.get(*line_idx)).map_or_else(
                    || {
                        Line::from(text)
                            .style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD))
                    },
                    |wl| render_word_line(wl, cursor.position_ms, theme),
                )
            } else {
                // 距中心越远越淡入背景。远端 endpoint 用 surface0,任何 bg 色下都是淡出。
                let fade = lerp_color(theme.subtext, theme.surface0, dist.saturating_sub(1), denom);
                Line::from(text).style(Style::new().fg(fade))
            }
        }
    }
}

/// 按 `position_ms` 把逐字行渲染成字符级渐变 Span 序列(KTV wipe)。
///
/// `Word.text` 对中文是单字、对英文是整词,所以在 `Word` 内再按 `text.chars()` 等分
/// 时间,每个 Unicode 字符独立 lerp 颜色,得到逐字渐变效果。
fn render_word_line<'a>(word_line: &'a WordLine, position_ms: u64, theme: &Theme) -> Line<'a> {
    let mut spans = Vec::<Span<'a>>::new();
    for w in &word_line.words {
        push_char_spans(&mut spans, w, position_ms, theme);
    }
    Line::from(spans)
}

/// 把一个 `Word` 按字符均分时间,每字符一个 Span,颜色按 char 内进度 lerp。
fn push_char_spans<'a>(
    out: &mut Vec<Span<'a>>,
    word: &'a mineral_model::Word,
    position_ms: u64,
    theme: &Theme,
) {
    let n = word.text.chars().count();
    let Ok(n_u64) = u64::try_from(n) else {
        return;
    };
    if n_u64 == 0 {
        return;
    }
    let total_dur = word.dur_ms.max(1);
    let mut byte_cursor = 0usize;
    for (i, ch) in word.text.chars().enumerate() {
        let Ok(i_u64) = u64::try_from(i) else {
            return;
        };
        let char_start = word.start_ms.saturating_add(i_u64 * total_dur / n_u64);
        let char_end = word
            .start_ms
            .saturating_add((i_u64 + 1) * total_dur / n_u64);
        let char_dur = char_end.saturating_sub(char_start).max(1);
        let elapsed = position_ms.saturating_sub(char_start).min(char_dur);
        // wipe 起点用 subtext(跟 d=1 邻居同亮度,中心行未唱部分不再「比周围暗」);
        // 终点 accent 跟 lrc 兜底中心行同色;整行加 BOLD 让最亮部分再亮一点。
        let color = lerp_color(theme.subtext, theme.accent, elapsed, char_dur);

        let next_byte = byte_cursor.saturating_add(ch.len_utf8());
        let slice = word.text.get(byte_cursor..next_byte).unwrap_or_default();
        byte_cursor = next_byte;

        out.push(Span::styled(
            slice,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use ratatui::text::Span;

    use super::{title_left_spans, title_right_spans};
    use crate::runtime::state::{AppState, LyricExtra};

    /// 拼接 Span 序列的纯文本(忽略样式),给标题文本形状断言用。
    fn text_of(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }
    use crate::render::theme::Theme;

    /// 无当前歌 / 无歌词缓存 → fallback 态。
    #[test]
    fn lyrics_fallback_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = AppState::empty();
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!("歌词:无当前歌 / 无缓存 → fallback", t.backend());
        Ok(())
    }

    /// 逐字 + 翻译档:每个可见原文行下方紧跟翻译副行,标识 `synced ✦ · 译`。
    #[test]
    fn lyrics_words_with_translation_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 14))?;
        let state = crate::test_support::state_with_lyrics(
            LyricExtra::Translation,
            /*with_words*/ true,
        );
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!("歌词:逐字 + 翻译副行(所有可见行)", t.backend());
        Ok(())
    }

    /// 行级(无逐字)+ 罗马音档:标识 `synced · 音`,每行下方紧跟罗马音副行。
    #[test]
    fn lyrics_lrc_with_romanization_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 14))?;
        let state = crate::test_support::state_with_lyrics(
            LyricExtra::Romanization,
            /*with_words*/ false,
        );
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!("歌词:行级 + 罗马音副行", t.backend());
        Ok(())
    }

    /// 无翻译 / 无罗马音(《飞鱼转身》)→ 右上不显示 `[t]` 提示,只有左上数据档。
    #[test]
    fn lyrics_no_extra_hides_hint_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 14))?;
        let state = crate::test_support::state_with_lrc_only();
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!("歌词:无副歌词 → 右上无 [t] 提示", t.backend());
        Ok(())
    }

    /// 左上数据档三档(文本形状;颜色不进文本快照)。
    #[test]
    fn title_left_tiers() {
        let th = Theme::default();
        assert_eq!(text_of(&title_left_spans(false, false, &th)), " lyrics ");
        assert_eq!(
            text_of(&title_left_spans(false, true, &th)),
            " lyrics · synced "
        );
        assert_eq!(
            text_of(&title_left_spans(true, true, &th)),
            " lyrics · synced ✦ "
        );
    }

    /// 右上副歌词档 + 按键提示;无副歌词时为空(不显示)。
    #[test]
    fn title_right_hint() {
        let th = Theme::default();
        assert_eq!(text_of(&title_right_spans(false, None, &th)), "");
        assert_eq!(
            text_of(&title_right_spans(
                false,
                Some(LyricExtra::Translation),
                &th
            )),
            ""
        );
        assert_eq!(text_of(&title_right_spans(true, None, &th)), " [t] ");
        assert_eq!(
            text_of(&title_right_spans(true, Some(LyricExtra::None), &th)),
            " [t] "
        );
        assert_eq!(
            text_of(&title_right_spans(true, Some(LyricExtra::Translation), &th)),
            " tr · [t] "
        );
        assert_eq!(
            text_of(&title_right_spans(
                true,
                Some(LyricExtra::Romanization),
                &th
            )),
            " ro · [t] "
        );
    }
}

//! Lyrics 面板:按 [`crate::state::AppState::current_lyrics`] 渲染当前行 + 邻近行,
//! 当前行高亮居中,上下各若干行 dim。无歌词时 fallback "♪ no lyrics"。
//!
//! 有 YRC(逐字)时,中心行走字级 wipe 渲染:已唱的字 = `theme.text` + Bold,
//! 未唱的字 = `theme.overlay` dim。邻行无论是否有 yrc 都按整行 dim 渲染。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::color::lerp_color;
use crate::lrc;
use crate::state::AppState;
use crate::theme::Theme;
use crate::yrc::{self, YrcLine};

/// 渲染 lyrics 面板到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" lyrics · synced ").style(Style::new().fg(theme.subtext)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let position_ms = state.playback.position_ms;
    // 有 yrc 时整个窗口都从 yrc 推:中心行 + 上下行同源,index 唯一,wipe 不会跟邻行错位。
    // 没 yrc 时回落到 lrc 行级整行高亮(原始路径)。
    let yrc_lines = state.current_yrc().filter(|v| !v.is_empty());
    let (lines, cur, yrc) = if let Some(yl) = yrc_lines {
        let lines: Vec<(u64, String)> = yl
            .iter()
            .map(|l| {
                let text: String = l.chars.iter().map(|c| c.text.as_str()).collect();
                (l.start_ms, text)
            })
            .collect();
        let cur = yrc::current_index(yl, position_ms);
        let yrc = YrcCursor {
            lines: Some(yl),
            cur,
            position_ms,
        };
        (lines, cur, yrc)
    } else {
        let Some(lrc_lines) = state.current_lyrics().filter(|v| !v.is_empty()) else {
            draw_fallback(frame, inner, theme);
            return;
        };
        let cur = lrc::current_index(lrc_lines, position_ms);
        let yrc = YrcCursor {
            lines: None,
            cur: None,
            position_ms,
        };
        (lrc_lines.clone(), cur, yrc)
    };
    paint_window(frame, inner, &lines, cur, yrc, theme);
}

/// 渲染时传入的 yrc 上下文(打包以减少 paint_window 参数数)。
#[derive(Clone, Copy)]
struct YrcCursor<'a> {
    lines: Option<&'a Vec<YrcLine>>,
    cur: Option<usize>,
    position_ms: u64,
}

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

/// 渲染以 `cur` 为中心、上下各 `(area.height - 1) / 2` 行的歌词窗口。
///
/// 中心行(`row == center_row`)若有 yrc 走字级 wipe,否则整行高亮;邻行整行 dim。
fn paint_window(
    frame: &mut Frame<'_>,
    inner: Rect,
    lines: &[(u64, String)],
    cur: Option<usize>,
    yrc: YrcCursor<'_>,
    theme: &Theme,
) {
    let height = usize::from(inner.height);
    let cur_idx = cur.unwrap_or(0);
    let center_row = height / 2;
    // fade 用:从中心向两侧最远距离的较大者(窗口可能不对称)。
    // 距离 1 时 fade 输入 num=0 → 满 subtext;最远时 num=denom → 满 surface1。
    let max_dist = u64::try_from(center_row.max(height.saturating_sub(center_row + 1)))
        .unwrap_or(0)
        .max(1);

    for row in 0..height {
        // 把行号映射到 lines 的 index:row=center_row 对应 cur_idx。
        let line_idx_signed = isize::try_from(row).unwrap_or(0)
            - isize::try_from(center_row).unwrap_or(0)
            + isize::try_from(cur_idx).unwrap_or(0);
        if line_idx_signed < 0 {
            continue;
        }
        let Ok(line_idx) = usize::try_from(line_idx_signed) else {
            continue;
        };
        let Some((_, text)) = lines.get(line_idx) else {
            continue;
        };

        let row_u16 = u16::try_from(row).unwrap_or(0);
        let row_area = Rect::new(inner.x, inner.y + row_u16, inner.width, 1);

        let is_center = Some(line_idx) == cur;
        let line: Line<'_> = if is_center {
            // 中心行用 accent(mauve)区别于灰阶 fade,色相差让聚焦行一眼可辨。
            yrc.lines
                .zip(yrc.cur)
                .and_then(|(v, i)| v.get(i))
                .map_or_else(
                    || {
                        Line::from(text.as_str())
                            .style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD))
                    },
                    |yl| render_yrc_line(yl, yrc.position_ms, theme),
                )
        } else {
            // 距中心越远越淡入背景。dark 端用 surface0 比 surface1 更暗,
            // 强化「中心聚焦、外围基本看不到」的对比。
            let dist_signed =
                isize::try_from(row).unwrap_or(0) - isize::try_from(center_row).unwrap_or(0);
            let dist = u64::try_from(dist_signed.unsigned_abs()).unwrap_or(0);
            // 远端 endpoint 用 surface0 —— mantle 在透明终端上反而成「显眼的黑字」,
            // surface1 又稍亮;surface0 中性偏暗,任何 bg 色下都是 fade-in-background。
            let fade = lerp_color(
                theme.subtext,
                theme.surface0,
                dist.saturating_sub(1),
                max_dist.saturating_sub(1).max(1),
            );
            Line::from(text.as_str()).style(Style::new().fg(fade))
        };
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), row_area);
    }
}

/// 按 `position_ms` 把 YRC 行渲染成字符级渐变 Span 序列(KTV wipe)。
///
/// 网易 yrc 的 `YrcChar.text` 对中文是单字、对英文是整词,所以在 `YrcChar` 内再按
/// `text.chars()` 等分时间,每个 Unicode 字符独立 lerp 颜色,得到逐字渐变效果。
fn render_yrc_line<'a>(yrc_line: &'a YrcLine, position_ms: u64, theme: &Theme) -> Line<'a> {
    let mut spans = Vec::<Span<'a>>::new();
    for c in &yrc_line.chars {
        push_char_spans(&mut spans, c, position_ms, theme);
    }
    Line::from(spans)
}

/// 把一个 `YrcChar` 按字符均分时间,每字符一个 Span,颜色按 char 内进度 lerp。
fn push_char_spans<'a>(
    out: &mut Vec<Span<'a>>,
    yrc_char: &'a crate::yrc::YrcChar,
    position_ms: u64,
    theme: &Theme,
) {
    let n = yrc_char.text.chars().count();
    let Ok(n_u64) = u64::try_from(n) else {
        return;
    };
    if n_u64 == 0 {
        return;
    }
    let total_dur = yrc_char.dur_ms.max(1);
    let mut byte_cursor = 0usize;
    for (i, ch) in yrc_char.text.chars().enumerate() {
        let Ok(i_u64) = u64::try_from(i) else {
            return;
        };
        let char_start = yrc_char.start_ms.saturating_add(i_u64 * total_dur / n_u64);
        let char_end = yrc_char
            .start_ms
            .saturating_add((i_u64 + 1) * total_dur / n_u64);
        let char_dur = char_end.saturating_sub(char_start).max(1);
        let elapsed = position_ms.saturating_sub(char_start).min(char_dur);
        // wipe 起点用 subtext(跟 d=1 邻居同亮度,中心行未唱部分不再「比周围暗」);
        // 终点 accent 跟 lrc 兜底中心行同色;整行加 BOLD 让最亮部分再亮一点。
        let color = lerp_color(theme.subtext, theme.accent, elapsed, char_dur);

        let next_byte = byte_cursor.saturating_add(ch.len_utf8());
        let slice = yrc_char
            .text
            .get(byte_cursor..next_byte)
            .unwrap_or_default();
        byte_cursor = next_byte;

        out.push(Span::styled(
            slice,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
}

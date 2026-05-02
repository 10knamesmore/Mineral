//! Lyrics 面板:按 [`crate::state::AppState::current_lyrics`] 渲染当前行 + 邻近行,
//! 当前行高亮居中,上下各若干行 dim。无歌词时 fallback "♪ no lyrics"。

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::lrc;
use crate::state::AppState;
use crate::theme::Theme;

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

    let lyrics = state.current_lyrics();
    let lines: Option<&Vec<(u64, String)>> = lyrics.filter(|v| !v.is_empty());
    let Some(lines) = lines else {
        draw_fallback(frame, inner, theme);
        return;
    };

    let cur = lrc::current_index(lines, state.playback.position_ms);
    paint_window(frame, inner, lines, cur, theme);
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

/// 渲染以 `cur` 为中心、上下各 `(area.height - 1) / 2` 行的歌词窗口,
/// 当前行加粗且用 theme.text,邻行 dim。
fn paint_window(
    frame: &mut Frame<'_>,
    inner: Rect,
    lines: &[(u64, String)],
    cur: Option<usize>,
    theme: &Theme,
) {
    let height = usize::from(inner.height);
    let cur_idx = cur.unwrap_or(0);
    let center_row = height / 2;

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

        let style = if Some(line_idx) == cur {
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.overlay)
        };
        let row_u16 = u16::try_from(row).unwrap_or(0);
        let row_area = Rect::new(inner.x, inner.y + row_u16, inner.width, 1);
        let line = Line::from(text.as_str()).style(style);
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), row_area);
    }
}

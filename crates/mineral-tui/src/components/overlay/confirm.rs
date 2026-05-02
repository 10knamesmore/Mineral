//! Quit 确认 modal:小尺寸居中,y/Enter 退出 / n/Esc 取消。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::components::overlay::centered_rect;
use crate::theme::Theme;

/// 渲染 quit confirm modal。`area` 是主帧区域。
pub fn draw(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let panel = centered_rect(area, 35, 30, 36, 7, 64, 9);
    paint_shadow(frame, panel, area, theme);
    frame.render_widget(Clear, panel);

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .style(Style::new().bg(theme.mantle))
        .title(Line::from(" quit tuimu? ").style(Style::new().fg(theme.subtext)));
    let inner = block.inner(panel);
    frame.render_widget(block, panel);

    if inner.height < 3 || inner.width < 12 {
        return;
    }

    let mid_y = inner.y + inner.height / 2;
    // 上半:消息(居中,文字色)。
    let msg_area = Rect::new(inner.x, mid_y.saturating_sub(1), inner.width, 1);
    let msg = Line::from("exit playback and close session?").style(Style::new().fg(theme.text));
    frame.render_widget(Paragraph::new(msg).alignment(Alignment::Center), msg_area);

    // 下半:选项(y red bold / n subtext bold)。
    let opts_y = mid_y.saturating_add(1).min(inner.y + inner.height - 1);
    let opts_area = Rect::new(inner.x, opts_y, inner.width, 1);
    let opts = Line::from(vec![
        Span::styled(
            "[ y ]",
            Style::new().fg(theme.red).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" yes      ", Style::new().fg(theme.subtext)),
        Span::styled(
            "[ n / esc ]",
            Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cancel", Style::new().fg(theme.subtext)),
    ]);
    frame.render_widget(Paragraph::new(opts).alignment(Alignment::Center), opts_area);
}

fn paint_shadow(frame: &mut Frame<'_>, panel: Rect, area: Rect, theme: &Theme) {
    let off_x = panel.x.saturating_add(2);
    let off_y = panel.y.saturating_add(1);
    let max_x = area.x.saturating_add(area.width);
    let max_y = area.y.saturating_add(area.height);
    if off_x >= max_x || off_y >= max_y {
        return;
    }
    let w = panel.width.min(max_x.saturating_sub(off_x));
    let h = panel.height.min(max_y.saturating_sub(off_y));
    let shadow = Rect::new(off_x, off_y, w, h);
    let bg = Block::new().style(Style::new().bg(theme.crust));
    frame.render_widget(Clear, shadow);
    frame.render_widget(bg, shadow);
}

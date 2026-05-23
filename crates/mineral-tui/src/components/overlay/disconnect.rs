//! daemon 断连提示 modal:与播放服务的链路断了(daemon 被单独 kill / crash)时
//! 盖在最上层,居中显示话术 +「按任意键退出」,停在那等用户按键。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::components::overlay::centered_rect;
use crate::theme::Theme;

/// 渲染断连提示 modal。`area` 是主帧区域。
pub fn draw(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let panel = centered_rect(area, 40, 30, 40, 7, 64, 9);
    frame.render_widget(Clear, panel);

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.red))
        .style(Style::new().bg(theme.mantle))
        .title(Line::from(" connection lost ").style(Style::new().fg(theme.red)));
    let inner = block.inner(panel);
    frame.render_widget(block, panel);

    if inner.height < 3 || inner.width < 12 {
        return;
    }

    let mid_y = inner.y + inner.height / 2;
    // 上半:话术(居中,文字色)。
    let msg_area = Rect::new(inner.x, mid_y.saturating_sub(1), inner.width, 1);
    let msg = Line::from("Lost connection to playback service").style(Style::new().fg(theme.text));
    frame.render_widget(Paragraph::new(msg).alignment(Alignment::Center), msg_area);

    // 下半:退出提示(居中,弱化色)。
    let hint_y = mid_y.saturating_add(1).min(inner.y + inner.height - 1);
    let hint_area = Rect::new(inner.x, hint_y, inner.width, 1);
    let hint = Line::from("Press any key to exit")
        .style(Style::new().fg(theme.subtext).add_modifier(Modifier::DIM));
    frame.render_widget(Paragraph::new(hint).alignment(Alignment::Center), hint_area);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::theme::Theme;

    /// 断连提示 modal 的渲染快照:话术 + 退出提示居中,带红框。
    #[test]
    fn disconnect_overlay_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(60, 12))?;
        let theme = Theme::default();
        terminal.draw(|f| super::draw(f, f.area(), &theme))?;
        insta::assert_snapshot!(terminal.backend());
        Ok(())
    }
}

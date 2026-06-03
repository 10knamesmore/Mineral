//! Quit 确认 modal:小尺寸居中,y/Enter 退出 / n/Esc 取消。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block,
};
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 退出确认 modal。无 UI-local 状态。
pub(crate) struct ConfirmOverlay;

impl Overlay for ConfirmOverlay {
    fn chrome(&self) -> Chrome {
        Chrome {
            pct_w: 35,
            pct_h: 30,
            min_w: 36,
            min_h: 7,
            max_w: 64,
            max_h: 9,
            animated: true,
            dock: false,
        }
    }

    fn block(&self, _ctx: &AppState, theme: &Theme, _focused: bool) -> Block<'static> {
        base_block(theme)
            .border_style(Style::new().fg(theme.accent))
            .title(Line::from(" quit mineral? ").style(Style::new().fg(theme.subtext)))
    }

    fn render_content(&self, frame: &mut Frame<'_>, inner: Rect, _ctx: &AppState, theme: &Theme) {
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

    fn on_key(&mut self, key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => OverlayResponse::Do(OverlayAction::Quit),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => OverlayResponse::Do(OverlayAction::CloseTop),
            // 模态:吞掉其余按键,不穿透。
            _ => OverlayResponse::Consumed,
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::ConfirmOverlay;
    use crate::components::popup::component::render_overlay;
    use crate::render::theme::Theme;
    use crate::runtime::state::AppState;

    /// quit confirm modal 渲染快照(完全展开)。
    #[test]
    fn confirm_overlay_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(60, 12))?;
        let ctx = AppState::empty();
        let overlay = ConfirmOverlay;
        terminal.draw(|f| {
            render_overlay(
                f,
                f.area(),
                &overlay,
                /*scale*/ 1000,
                /*focused*/ true,
                &ctx,
                &Theme::default(),
            );
        })?;
        crate::test_support::assert_snap!("退出确认 modal(y 确认 / n 取消)", terminal.backend());
        Ok(())
    }
}

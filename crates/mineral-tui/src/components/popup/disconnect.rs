//! daemon 断连提示 modal:与播放服务的链路断了(daemon 被单独 kill / crash)时
//! 盖在最上层,居中显示话术 +「按任意键退出」,停在那等用户按键。

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Widget};

use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block,
};
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 断连提示 modal。无 UI-local 状态;任意键退出。
pub(crate) struct DisconnectOverlay;

impl Overlay for DisconnectOverlay {
    fn chrome(&self) -> Chrome {
        Chrome {
            pct_w: 40,
            pct_h: 30,
            min_w: 40,
            min_h: 7,
            max_w: 64,
            max_h: 9,
            animated: true,
            dock: false,
        }
    }

    fn block(&self, _ctx: &AppState, theme: &Theme, _focused: bool) -> Block<'static> {
        base_block(theme)
            .border_style(Style::new().fg(theme.red))
            .title(Line::from(" connection lost ").style(Style::new().fg(theme.red)))
    }

    fn render_content(&self, buf: &mut Buffer, inner: Rect, _ctx: &AppState, theme: &Theme) {
        if inner.height < 3 || inner.width < 12 {
            return;
        }
        let mid_y = inner.y + inner.height / 2;
        // 上半:话术(居中,文字色)。
        let msg_area = Rect::new(inner.x, mid_y.saturating_sub(1), inner.width, 1);
        let msg =
            Line::from("Lost connection to playback service").style(Style::new().fg(theme.text));
        Paragraph::new(msg)
            .alignment(Alignment::Center)
            .render(msg_area, buf);

        // 下半:退出提示(居中,弱化色)。
        let hint_y = mid_y.saturating_add(1).min(inner.y + inner.height - 1);
        let hint_area = Rect::new(inner.x, hint_y, inner.width, 1);
        let hint = Line::from("Press any key to exit")
            .style(Style::new().fg(theme.subtext).add_modifier(Modifier::DIM));
        Paragraph::new(hint)
            .alignment(Alignment::Center)
            .render(hint_area, buf);
    }

    fn on_key(&mut self, _key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        // 链路已断,正常路径全是兜底默认值:任意键直接退出。
        OverlayResponse::Do(OverlayAction::Quit)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::DisconnectOverlay;
    use crate::components::popup::component::render_overlay;
    use crate::render::theme::Theme;
    use crate::runtime::state::AppState;

    /// 断连提示 modal 的渲染快照:话术 + 退出提示居中,带红框(完全展开)。
    #[test]
    fn disconnect_overlay_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(60, 12))?;
        let ctx = AppState::test_default()?;
        let overlay = DisconnectOverlay;
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
        crate::test_support::assert_snap!("daemon 断连提示 modal(等按键退出)", terminal.backend());
        Ok(())
    }
}

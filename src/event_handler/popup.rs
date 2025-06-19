use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::{app::Context, event_handler::AppEvent, state::PopupState};

pub(super) fn handle_confirm_exit(ctx: &mut Context, key_event: KeyEvent) {
    if let KeyEventKind::Press = key_event.kind {
        match key_event.code {
            KeyCode::Char('y') | KeyCode::Char('q') => AppEvent::Quit.emit(),
            KeyCode::Char('n') => ctx.popup(PopupState::None),
            _ => {}
        }
    }
}

pub(super) fn handle_notification(ctx: &mut Context, key_event: KeyEvent) {
    if let KeyEventKind::Press = key_event.kind {
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('Q') => {
                ctx.consume_first_notification()
            }
            _ => {}
        }
    }
}

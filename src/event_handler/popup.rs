use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::{event_handler::AppEvent, state::PopupState, App};

pub(super) fn handle_confirm_exit(app: &mut App, key_event: KeyEvent) {
    if let KeyEventKind::Press = key_event.kind {
        match key_event.code {
            KeyCode::Char('y') | KeyCode::Char('q') => AppEvent::Quit.emit(),
            KeyCode::Char('n') => app.popup(PopupState::None),
            _ => {}
        }
    }
}

pub(super) fn handle_notification(app: &mut App, key_event: KeyEvent) {
    if let KeyEventKind::Press = key_event.kind {
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('Q') => {
                app.consume_first_notification()
            }
            _ => {}
        }
    }
}

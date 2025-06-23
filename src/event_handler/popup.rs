use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::event_handler::{action::PopupAction, Action};

pub(super) fn dispatch_confirm_exit(key_event: KeyEvent) -> Option<Action> {
    if let KeyEventKind::Press = key_event.kind {
        match key_event.code {
            KeyCode::Char('y') | KeyCode::Char('q') => Some(Action::Popup(PopupAction::ConfirmYes)),
            KeyCode::Char('n') => Some(Action::Popup(PopupAction::ConfirmNo)),
            _ => None,
        }
    } else {
        None
    }
}

pub(super) fn dispatch_notification(key_event: KeyEvent) -> Option<Action> {
    if let KeyEventKind::Press = key_event.kind {
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('Q') => {
                Some(Action::Popup(PopupAction::ClosePopup))
            }
            _ => None,
        }
    } else {
        None
    }
}

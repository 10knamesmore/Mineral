use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::event_handler::{action::PopupResponse, Action};

pub(super) fn dispatch_confirm_exit(key_event: KeyEvent) -> Option<Action> {
    if let KeyEventKind::Press = key_event.kind {
        match key_event.code {
            KeyCode::Char('y') | KeyCode::Char('q') => {
                Some(Action::PopupResponse(PopupResponse::ConfirmExit {
                    accepted: true,
                }))
            }
            KeyCode::Char('n') => Some(Action::PopupResponse(PopupResponse::ConfirmExit {
                accepted: false,
            })),
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
                Some(Action::PopupResponse(PopupResponse::ClosePopup))
            }
            _ => None,
        }
    } else {
        None
    }
}

use crossterm::event::{KeyCode, KeyEventKind};

use crate::{App, event_handler::AppEvent, state::PopupState};

pub(super) fn handle_confirm_exit(app: &mut App, event: AppEvent) {
    match event {
        AppEvent::Key(key_event) => {
            if let KeyEventKind::Press = key_event.kind {
                match key_event.code {
                    KeyCode::Char('y') | KeyCode::Char('q') => app.quit(),
                    KeyCode::Char('n') => app.popup(PopupState::None),
                    _ => {}
                }
            }
        }
        AppEvent::Resize(_, _) => {
            todo!("可能会处理Resize到比较小的时候,只显示播放进度等")
        }
        _ => {}
    }
}

pub(super) fn handle_notification(app: &mut App, event: AppEvent) {
    match event {
        AppEvent::Key(key_event) => {
            if let KeyEventKind::Press = key_event.kind {
                match key_event.code {
                    KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('Q') => {
                        app.consume_first_notification()
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

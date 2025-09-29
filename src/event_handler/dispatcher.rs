use crossterm::event::KeyEvent;

use crate::{
    event_handler::{
        global, main_page,
        popup::{dispatch_confirm_exit, dispatch_notification},
        Action,
    },
    state::{app_state::AppState, Page, PopupState},
};

pub fn dispatch_key(app_state: &AppState, key_event: KeyEvent) -> Option<Action> {
    match app_state.should_popup() {
        PopupState::ConfirmExit => dispatch_confirm_exit(key_event),
        PopupState::Notificacion => dispatch_notification(key_event),
        PopupState::None => {
            if let Some(global_action) = global::dispatch(&key_event) {
                return Some(global_action);
            }

            match app_state.now_page() {
                Page::Main => main_page::dispatch(key_event),
                _ => {
                    todo!("MainPage以外的event handle")
                }
            }
        }
    }
}

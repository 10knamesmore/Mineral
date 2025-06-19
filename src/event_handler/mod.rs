use crossterm::event::KeyEvent;
use global::handle_global_key;

use crate::{state::Page, state::PopupState, App};

mod app_events;
mod global;
mod main_page;
mod popup;

pub use app_events::*;

pub(crate) fn dispatch_key(app: &mut App, key_event: KeyEvent) {
    match app.should_popup() {
        PopupState::ConfirmExit => {
            popup::handle_confirm_exit(app, key_event);
        }
        PopupState::Notificacion => {
            popup::handle_notification(app, key_event);
        }
        PopupState::None => {
            if !handle_global_key(app, &key_event) {
                match app.now_page() {
                    Page::Main => {
                        main_page::handle_main_page_event(app, key_event);
                    }
                    _ => {
                        todo!("MainPage以外的event handle")
                    }
                }
            }
        }
    }
}

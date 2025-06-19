use global::handle_global_key;

use crate::{App, state::Page, state::PopupState};

mod app_events;
mod global;
mod main_page;
mod popup;

pub use app_events::*;

pub(crate) fn handle_event(app: &mut App, event: AppEvent) {
    match app.should_popup() {
        PopupState::ConfirmExit => {
            popup::handle_confirm_exit(app, event);
        }
        PopupState::Notificacion => {
            popup::handle_notification(app, event);
        }
        PopupState::None => {
            if !handle_global_key(app, &event) {
                match app.now_page() {
                    Page::Main => {
                        main_page::handle_main_page_event(app, event);
                    }
                    _ => {
                        todo!("MainPage以外的event handle")
                    }
                }
            }
        }
    }
}

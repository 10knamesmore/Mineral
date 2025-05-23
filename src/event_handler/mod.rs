use global::handle_global_key;
use ratatui::crossterm::event::{Event};

use crate::{App, state::Page, state::PopupState};

mod global;
mod main_page;
mod popup;

pub(crate) fn handle_event(app: &mut App, event: Event) {
    match app.should_popup() {
        PopupState::ConfirmExit => {
            popup::handle_confirm_exit(app, event);
        }
        PopupState::None => {
            if !handle_global_key(app, &event) {
                match app.get_now_page() {
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

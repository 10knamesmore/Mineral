mod action;
mod dispatcher;
mod events;
mod global;
mod main_page;
mod popup;

pub use action::*;
pub use dispatcher::dispatch_key;
pub use events::*;

use crate::{
    event_handler::action::PageAction,
    state::{app_state::AppState, Page},
};

pub fn handle_page_action(app_state: &mut AppState, action: PageAction) {
    match app_state.now_page() {
        Page::Main => main_page::handle_page_action(app_state.mut_main_page(), action),
        Page::Search => todo!(),
    }
    AppEvent::Render.emit();
}

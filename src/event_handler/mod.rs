mod action;
mod dispatcher;
mod events;
mod global;
mod main_page;
mod popup;

pub use action::*;
pub use dispatcher::dispatch_key;
pub use events::*;

use crate::{app::Context, event_handler::action::PageAction, state::Page};

pub fn handle_page_action(ctx: &mut Context, action: PageAction) {
    match ctx.now_page() {
        Page::Main => main_page::handle_page_action(ctx.mut_main_page(), action),
        Page::Search => todo!(),
    }
}

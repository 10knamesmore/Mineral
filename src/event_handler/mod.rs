use crossterm::event::KeyEvent;
use global::handle_global_key;

use crate::{
    app::Context,
    state::{Page, PopupState},
};

mod app_events;
mod global;
mod main_page;
mod popup;

pub use app_events::*;

pub(crate) fn dispatch_key(ctx: &mut Context, key_event: KeyEvent) {
    match ctx.should_popup() {
        PopupState::ConfirmExit => {
            popup::handle_confirm_exit(ctx, key_event);
        }
        PopupState::Notificacion => {
            popup::handle_notification(ctx, key_event);
        }
        PopupState::None => {
            if !handle_global_key(ctx, &key_event) {
                match ctx.now_page() {
                    Page::Main => {
                        main_page::handle_main_page_event(ctx, key_event);
                    }
                    _ => {
                        todo!("MainPage以外的event handle")
                    }
                }
            }
        }
    }
}

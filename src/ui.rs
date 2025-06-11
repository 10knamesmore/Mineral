use crate::{
    app::{App, RenderCache},
    state::{Page, PopupState},
};
use main_page::*;
use ratatui::Frame;

mod components;
mod main_page;
mod popup;

pub(crate) fn render_ui(app: &App, frame: &mut Frame, cache: &mut RenderCache) {
    match app.get_now_page() {
        Page::Main => {
            draw_main_page(app, frame, cache);
        }
        _ => {
            todo!("其它页面的ui render")
        }
    }

    match app.should_popup() {
        PopupState::ConfirmExit => {
            popup::confirm_exit(frame);
        }
        PopupState::None => {}
        PopupState::Notificacion => popup::notify(
            app.first_notification()
                .expect("程序内部错误: PopupState 为Notification 但当前队列中不存在消息 "),
            frame,
        ),
    }
}

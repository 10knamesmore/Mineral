use crate::{
    app::RenderCache,
    state::{app_state::AppState, Page, PopupState},
};
use main_page::*;
use ratatui::Frame;

mod main_page;
mod popup;

pub(crate) fn render_ui(app_state: &AppState, frame: &mut Frame, cache: &mut RenderCache) {
    match app_state.now_page() {
        Page::Main => {
            draw_main_page(app_state, frame, cache);
        }
        _ => {
            todo!("其它页面的ui render")
        }
    }

    match app_state.should_popup() {
        PopupState::ConfirmExit => {
            popup::confirm_exit(frame);
        }
        PopupState::None => {}
        PopupState::Notificacion => popup::notify(
            app_state
                .first_notification()
                .expect("程序内部错误: PopupState 为Notification 但当前队列中不存在消息 "),
            frame,
        ),
    }
}

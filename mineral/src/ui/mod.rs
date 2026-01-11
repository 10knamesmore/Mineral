use crate::{
    app::{Context, RenderCache},
    state::{Page, PopupState},
};
use main_page::*;
use ratatui::Frame;

mod componnents;
mod main_page;
mod page;
mod popup;

pub(crate) fn render_ui(ctx: &Context, frame: &mut Frame, cache: &mut RenderCache) {
    match ctx.now_page() {
        Page::Main => {
            draw_main_page(ctx, frame, cache);
        }
        _ => {
            todo!("其它页面的ui render")
        }
    }

    match ctx.should_popup() {
        PopupState::ConfirmExit => {
            popup::confirm_exit(frame);
        }
        PopupState::None => {}
        PopupState::Notificacion => popup::notify(
            ctx.first_notification()
                .expect("程序内部错误: PopupState 为Notification 但当前队列中不存在消息 "),
            frame,
        ),
    }
}

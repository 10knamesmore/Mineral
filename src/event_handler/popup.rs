use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind};

use crate::{App, state::PopupState};

pub(super) fn handle_confirm_exit(app: &mut App, event: Event) {
    match event {
        Event::Key(key_event) => {
            if let KeyEventKind::Press = key_event.kind {
                match key_event.code {
                    KeyCode::Char('y') | KeyCode::Char('q') => app.quit(),
                    KeyCode::Char('n') => app.popup(PopupState::None),
                    _ => {}
                }
            }
        }
        Event::Mouse(mouse_event) => {
            todo!("可能会允许鼠标点击退出")
        }
        Event::Resize(_, _) => {
            todo!("可能会处理Resize到比较小的时候,只显示播放进度等")
        }
        _ => {}
    }
}

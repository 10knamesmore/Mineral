use crossterm::event::KeyCode;

use crate::{App, event_handler::AppEvent};

pub(super) fn handle_main_page_event(app: &mut App, event: AppEvent) {
    match event {
        AppEvent::Key(key_event) => match key_event.code {
            KeyCode::Char('k') | KeyCode::Up => {
                app.table_move_up_by(1);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.table_move_down_by(1);
            }
            KeyCode::Char('K') => {
                app.table_move_up_by(5);
            }
            KeyCode::Char('J') => {
                app.table_move_down_by(5);
            }
            KeyCode::Enter | KeyCode::Char('l') => app.nav_forward(),
            KeyCode::Char('h') => app.nav_backward(),
            KeyCode::Char('H') => {
                unimplemented!("切换主页面的 Page ")
            }
            KeyCode::Char('L') => {
                unimplemented!("切换主页面的 Page ")
            }
            _ => {}
        },
        AppEvent::Resize(_, _) => {
            // TODO: 处理窗口大小变化
        }
        _ => {}
    }
}

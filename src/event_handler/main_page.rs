use crossterm::event::{KeyCode, KeyEvent};

use crate::App;

pub(super) fn handle_main_page_event(app: &mut App, key_event: KeyEvent) {
    match key_event.code {
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
    }
}

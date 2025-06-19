use crossterm::event::{KeyCode, KeyEvent};

use crate::app::Context;

pub(super) fn handle_main_page_event(ctx: &mut Context, key_event: KeyEvent) {
    match key_event.code {
        KeyCode::Char('k') | KeyCode::Up => {
            ctx.table_move_up_by(1);
        }
        KeyCode::Char('j') | KeyCode::Down => {
            ctx.table_move_down_by(1);
        }
        KeyCode::Char('K') => {
            ctx.table_move_up_by(5);
        }
        KeyCode::Char('J') => {
            ctx.table_move_down_by(5);
        }
        KeyCode::Enter | KeyCode::Char('l') => ctx.nav_forward(),
        KeyCode::Char('h') => ctx.nav_backward(),
        KeyCode::Char('H') => {
            unimplemented!("切换主页面的 Page ")
        }
        KeyCode::Char('L') => {
            unimplemented!("切换主页面的 Page ")
        }
        _ => {}
    }
}

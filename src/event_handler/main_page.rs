use ratatui::crossterm::event::{Event, KeyCode};

use crate::App;

pub(super) fn handle_main_page_event(app: &mut App, event: Event) {
    match event {
        Event::Key(key_event) => match key_event.code {
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
            KeyCode::Enter | KeyCode::Char('l') => {
                unimplemented!("Enter or 'l' key pressed");
            }
            _ => {}
        },
        Event::Mouse(_) => {
            // TODO: 鼠标支持
        }
        Event::Paste(_) => {
            // TODO: 粘贴支持
        }
        Event::Resize(_, _) => {
            // TODO: 处理窗口大小变化
        }
        _ => {}
    }
}

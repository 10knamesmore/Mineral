use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::{
    event_handler::Action,
    util::notification::{Notification, NotifyUrgency},
};

pub fn dispatch(key_event: &KeyEvent) -> Option<Action> {
    if KeyEventKind::Press == key_event.kind {
        match key_event.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('?') => Some(Action::Help),
            KeyCode::Char('n') => {
                let title = String::from("Test");
                let content = format!("测试! {} at {}", file!(), line!());
                let urgency = NotifyUrgency::Debug;
                Some(Action::Notification(Notification::new(
                    title, content, urgency,
                )))
            }
            _ => None,
        }
    } else {
        None
    }
}

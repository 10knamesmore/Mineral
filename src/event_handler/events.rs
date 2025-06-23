use crossterm::event::KeyEvent;
use once_cell::sync::OnceCell;
use tokio::sync::mpsc::UnboundedSender;

use crate::event_handler::Action;

static TX: OnceCell<UnboundedSender<AppEvent>> = OnceCell::new();

pub enum AppEvent {
    Exit,
    Key(KeyEvent),
    Resize(u16, u16),
    Action(Action),
}

impl AppEvent {
    pub fn init(tx: UnboundedSender<AppEvent>) {
        TX.set(tx).expect("AppEvent sender 只应当被初始化一次!");
    }

    pub fn emit(self) {
        if let Some(tx) = TX.get() {
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(self);
            });
        } else {
            eprintln!("AppEvent sender 没有被初始化!");
        }
    }
}

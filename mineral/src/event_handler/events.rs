use crossterm::event::KeyEvent;
use once_cell::sync::OnceCell;
use tokio::sync::mpsc::UnboundedSender;

use crate::event_handler::Action;

static TX: OnceCell<UnboundedSender<AppEvent>> = OnceCell::new();

#[derive(Debug, strum_macros::Display)]
pub enum AppEvent {
    Exit,

    #[strum(to_string = "Key({0:?})")]
    Key(KeyEvent),

    #[strum(to_string = "Resize({0}, {1})")]
    Resize(u16, u16),

    #[strum(to_string = "Action({0})")]
    Action(Action),
    Render,
}

impl AppEvent {
    pub fn init(tx: UnboundedSender<AppEvent>) {
        TX.set(tx).expect("AppEvent sender 只应当被初始化一次!");
    }

    pub fn emit(self) {
        if let Some(tx) = TX.get() {
            let tx = tx.clone();
            let _ = tx.send(self);
            // tokio::spawn(async move {
            //     let _ = tx.send(self);
            // });
        } else {
            eprintln!("AppEvent sender 没有被初始化!");
        }
    }
}

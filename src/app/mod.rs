use crate::{
    app::{data_generator::test_render_cache, signals::Signals},
    event_handler::{self, handle_page_action, Action, AppEvent},
    ui::render_ui,
};
use ratatui::DefaultTerminal;
use std::io::{self};

mod cache;
mod context;
pub mod data_generator;
mod models;
mod signals;
mod style;

pub(crate) use cache::*;
pub(crate) use context::*;
pub(crate) use models::*;
pub(crate) use style::*;

pub(crate) struct App {
    ctx: Context,
    signals: Signals,
}

impl App {
    pub(crate) async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        // HACK: 正式运行更改
        let mut cache = test_render_cache();
        loop {
            terminal.draw(|frame| {
                render_ui(&self.ctx, frame, &mut cache);
            })?;

            if let Some(event) = self.signals.rx.recv().await {
                match event {
                    AppEvent::Action(Action::Quit) => break,
                    AppEvent::Key(key_event) => {
                        if let Some(action) = event_handler::dispatch_key(&self.ctx, key_event) {
                            AppEvent::Action(action).emit();
                        }
                    }
                    AppEvent::Resize(_, _) => todo!(),
                    AppEvent::Action(action) => self.handle(action).await,
                }
            }
        }

        Ok(())
    }

    async fn handle(&mut self, action: Action) {
        match action {
            Action::Quit => AppEvent::Action(Action::Quit).emit(),
            Action::Help => todo!(),
            Action::Notification(notification) => self.ctx.notify(notification),
            Action::Page(page_action) => handle_page_action(&mut self.ctx, page_action),
            Action::Popup(popup_action) => todo!(),
            Action::PlaySelectedTrac => todo!("handle 播放"),
        }
    }
}

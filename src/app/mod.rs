use crate::{
    app::{data_generator::test_render_cache, signals::Signals},
    event_handler::{self, AppEvent},
    ui::render_ui,
};
use ratatui::DefaultTerminal;
use std::io::{self};

mod cache;
mod context;
mod data_generator;
mod models;
mod signals;
mod style;

pub(crate) use cache::*;
pub(crate) use context::*;
pub(crate) use data_generator::test_struct_app;
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
                    AppEvent::Quit => break,
                    AppEvent::Key(key_event) => {
                        event_handler::dispatch_key(&mut self.ctx, key_event)
                    }
                    AppEvent::Resize(_, _) => todo!(),
                }
            }
        }

        Ok(())
    }
}

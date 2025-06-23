use crate::{
    app::{data_generator::test_render_cache, signals::Signals},
    event_handler::{self, handle_page_action, Action, AppEvent, PopupResponse},
    state::PopupState,
    ui::render_ui,
};
use ratatui::DefaultTerminal;
use std::{
    io::{self},
    sync::Arc,
    time::Duration,
};
use tokio::time::{self};
use tokio::{select, sync::Mutex};

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
        let cache: Arc<Mutex<RenderCache>> = test_render_cache();

        // 30hz
        let mut render_interval = time::interval(Duration::from_millis(33));
        let mut should_render = true;

        loop {
            select! {
                Some(event) = self.signals.rx.recv() => {
                    match event {
                        AppEvent::Exit => break,
                        AppEvent::Key(key_event) => {
                            if let Some(action) = event_handler::dispatch_key(&self.ctx, key_event) {
                                AppEvent::Action(action).emit();
                            }
                        }
                        AppEvent::Resize(_, _) => todo!(),
                        AppEvent::Action(action) => self.handle(action).await,
                        AppEvent::Render => {
                            should_render = true;
                        }
                    }
                }

                _ = render_interval.tick() => {
                    if should_render {
                        should_render = false;
                        let mut cache_guard = cache.lock().await;
                        terminal.draw(|frame| {
                            render_ui(&self.ctx, frame, &mut cache_guard);
                        })?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.ctx.popup(PopupState::ConfirmExit);
            }
            Action::Help => todo!(),
            Action::Notification(notification) => {
                self.ctx.notify(notification);
            }
            Action::Page(page_action) => handle_page_action(&mut self.ctx, page_action),
            Action::PopupResponse(popup_response) => match popup_response {
                PopupResponse::ConfirmExit { accepted } => {
                    if accepted {
                        AppEvent::Exit.emit();
                    } else {
                        self.ctx.popup(PopupState::None);
                    }
                }
                PopupResponse::ClosePopup => {
                    self.ctx.popup(PopupState::None);
                }
            },
            Action::PlaySelectedTrac => todo!("handle 播放"),
        }
    }
}

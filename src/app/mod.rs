use crate::{
    app::{config::Config, signals::Signals},
    event_handler::{self, handle_page_action, Action, AppEvent, PopupResponse},
    state::PopupState,
    ui::render_ui,
};
use anyhow::Ok;
use ratatui::DefaultTerminal;
use ratatui_image::picker::Picker;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::{io::BufReader, path::Path, sync::Arc, time::Duration};
use tokio::time::{self};
use tokio::{select, sync::Mutex};

mod cache;
mod config;
mod context;
pub mod logger;
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
    cfg: &'static Config,

    stream: Option<OutputStream>,
    stream_handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
}

impl App {
    pub(crate) async fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        // HACK: 正式运行更改
        let cache: Arc<Mutex<RenderCache>> = Self::render_cache();
        self.ctx.load_musics(self.cfg.music_dirs());

        // 30hz
        let mut render_interval = time::interval(Duration::from_millis(33));
        let mut should_render = true;

        loop {
            select! {
                Some(event) = self.signals.rx.recv() => {
                    tracing::trace!("接收Event: {}",&event);
                    match event {
                        AppEvent::Exit => break,
                        AppEvent::Key(key_event) => {
                            if let Some(action) = event_handler::dispatch_key(&self.ctx, key_event) {
                                AppEvent::Action(action).emit();
                            }
                        }
                        AppEvent::Resize(_, _) => todo!("handle resize"),
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
            Action::LoadMusics => {
                self.ctx.load_musics(self.cfg.music_dirs());
            }
            Action::PlaySong(song) => self
                .play(&song)
                .await
                .unwrap_or_else(|e| tracing::warn!("播放歌曲 {} 时发生错误: {}", song.name, e)),
        }
    }

    pub async fn play(&mut self, song: &Song) -> anyhow::Result<()> {
        if let Some(sink) = &self.sink {
            sink.stop();
        }

        let stream_handle = self
            .stream_handle
            .as_ref()
            .expect("stream_handle 未初始化!");

        let path = song
            .local_path
            .as_ref()
            .ok_or_else(|| todo!("无本地路径的歌曲"))
            .unwrap();

        let file = std::fs::File::open(path)?;
        let source = Decoder::new(BufReader::new(file))?;

        let sink = Sink::try_new(stream_handle)?;
        sink.append(source);
        sink.play();

        self.sink = Some(sink);

        Ok(())
    }

    pub fn init() -> anyhow::Result<App> {
        use anyhow::Context;

        let ctx = crate::app::Context::default();
        let cfg = crate::app::Config::get();
        let signals = Signals::start().context("初始化程序信号时发生错误")?;

        AppEvent::Render.emit();

        let (stream, stream_handle) = OutputStream::try_default()?;

        Ok(App {
            ctx,
            signals,
            cfg,
            stream: Some(stream),
            stream_handle: Some(stream_handle),
            sink: None,
        })
    }

    fn render_cache() -> Arc<Mutex<RenderCache>> {
        let picker = Picker::from_query_stdio().unwrap();
        let home_path = Path::new(std::env!("HOME"));
        let cache_path = home_path.join("Pictures/ncm_tui/");

        RenderCache::new(picker, cache_path)
    }
}

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream};
use futures_util::StreamExt;
use libc::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use tokio::{
    select,
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
};

use crate::event_handler::AppEvent;

pub struct Signals {
    pub tx: UnboundedSender<AppEvent>,
    pub rx: UnboundedReceiver<AppEvent>,

    term_stop_tx: Option<oneshot::Sender<()>>,
    term_stop_rx: Option<oneshot::Receiver<()>>,
}

impl Signals {
    pub fn start() -> Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();

        let (term_tx, term_rx) = oneshot::channel();

        let mut signals = Self {
            tx: tx.clone(),
            rx,
            term_stop_tx: Some(term_tx),
            term_stop_rx: Some(term_rx),
        };

        signals.spawn_system_task()?;
        signals.spawn_crossterm_task();

        AppEvent::init(tx);
        Ok(signals)
    }

    fn spawn_system_task(&self) -> Result<JoinHandle<()>> {
        let tx = self.tx.clone();
        let mut signals = signal_hook_tokio::Signals::new([SIGHUP, SIGTERM, SIGINT, SIGQUIT])?;

        Ok(tokio::spawn(async move {
            while let Some(signal) = signals.next().await {
                match signal {
                    SIGHUP | SIGTERM | SIGINT | SIGQUIT => {
                        if tx.send(AppEvent::Quit).is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }))
    }

    fn spawn_crossterm_task(&mut self) -> JoinHandle<()> {
        let tx = self.tx.clone();
        let mut stop_rx = self.term_stop_rx.take().unwrap();

        tokio::spawn(async move {
            let mut reader = EventStream::new();

            loop {
                select! {
                    _ = &mut stop_rx => break,
                    Some(Ok(event)) = reader.next() => {
                        let event = match event {
                            CrosstermEvent::Key(key) => AppEvent::Key(key),
                            CrosstermEvent::Resize(cols, rows) => AppEvent::Resize(cols, rows),
                            _ => continue,
                        };
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                }
            }
        })
    }

    pub fn stop_term(&mut self, state: bool) {
        todo!()
    }
}

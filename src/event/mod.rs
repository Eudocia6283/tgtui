pub mod handler;

use crate::backend::BackendEvent;
use crossterm::event::{Event as CrosstermEvent, EventStream};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub enum AppEvent {
    Input(CrosstermEvent),
    Backend(BackendEvent),
    /// SIGHUP / SIGTERM (e.g. the terminal window was closed).
    Quit,
}

pub struct EventHandler {
    tx: mpsc::UnboundedSender<AppEvent>,
    pub rx: mpsc::UnboundedReceiver<AppEvent>,
    input_task: Option<JoinHandle<()>>,
}

impl EventHandler {
    pub fn new(backend_rx: mpsc::UnboundedReceiver<BackendEvent>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        // Backend event forwarder
        let tx_backend = tx.clone();
        let mut b_rx = backend_rx;
        tokio::spawn(async move {
            while let Some(evt) = b_rx.recv().await {
                if tx_backend.send(AppEvent::Backend(evt)).is_err() {
                    break;
                }
            }
        });

        // Leave cleanly (and save UI state) when the terminal goes away.
        let tx_signal = tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let (Ok(mut hup), Ok(mut term)) = (signal(SignalKind::hangup()), signal(SignalKind::terminate()))
            else {
                return;
            };
            tokio::select! {
                _ = hup.recv() => {}
                _ = term.recv() => {}
            }
            let _ = tx_signal.send(AppEvent::Quit);
        });

        let mut handler = Self {
            tx,
            rx,
            input_task: None,
        };
        handler.start_input();
        handler
    }

    /// Start reading terminal input.
    pub fn start_input(&mut self) {
        if self.input_task.is_some() {
            return;
        }
        let tx = self.tx.clone();
        self.input_task = Some(tokio::spawn(async move {
            let mut reader = EventStream::new();
            while let Some(evt) = reader.next().await {
                if let Ok(evt) = evt {
                    if tx.send(AppEvent::Input(evt)).is_err() {
                        break;
                    }
                }
            }
        }));
    }

    /// Stop reading terminal input (e.g. while an external editor owns the
    /// terminal), otherwise the EventStream would steal its keystrokes.
    pub async fn stop_input(&mut self) {
        if let Some(task) = self.input_task.take() {
            task.abort();
            let _ = task.await;
        }
        // Drop input that was queued before the editor started.
        let mut pending = Vec::new();
        while let Ok(evt) = self.rx.try_recv() {
            if let AppEvent::Backend(b) = evt {
                pending.push(b);
            }
        }
        for b in pending {
            let _ = self.tx.send(AppEvent::Backend(b));
        }
    }
}

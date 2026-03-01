//! Terminal event handling -- crossterm events + async audio events.

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent};
use std::time::Duration;
use tokio::sync::mpsc;

/// Terminal events combined with async audio events.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Event {
    /// Terminal tick (for animations/updates).
    Tick,
    /// Key press.
    Key(KeyEvent),
    /// Terminal resize.
    Resize(u16, u16),
    /// Async event from the audio processing thread.
    Async(super::state::AsyncEvent),
}

/// Event handler that polls crossterm and a crossbeam async channel in a
/// background tokio task.
pub struct EventHandler {
    /// Event receiver.
    rx: mpsc::UnboundedReceiver<Event>,
    /// Stop signal sender (dropped to stop the background task).
    _stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl EventHandler {
    /// Create with tick rate and optional async event receiver from audio thread.
    ///
    /// The `async_rx` parameter receives `AsyncEvent` values from the audio
    /// processing thread via a crossbeam channel (which is thread-safe and
    /// works across the std thread / tokio boundary).
    pub fn new(
        tick_rate: Duration,
        async_rx: Option<crossbeam_channel::Receiver<super::state::AsyncEvent>>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    _ = async {
                        // Check for async events first (non-blocking)
                        if let Some(ref arx) = async_rx {
                            while let Ok(evt) = arx.try_recv() {
                                let _ = tx.send(Event::Async(evt));
                            }
                        }

                        if event::poll(tick_rate).unwrap_or(false) {
                            match event::read() {
                                Ok(CrosstermEvent::Key(key)) => {
                                    let _ = tx.send(Event::Key(key));
                                }
                                Ok(CrosstermEvent::Resize(w, h)) => {
                                    let _ = tx.send(Event::Resize(w, h));
                                }
                                _ => {}
                            }
                        } else {
                            let _ = tx.send(Event::Tick);
                        }
                    } => {}
                }
            }
        });

        Self {
            rx,
            _stop_tx: Some(stop_tx),
        }
    }

    /// Receive the next event.
    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

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

/// Crossterm event or tick, returned from the blocking poll thread.
enum TermEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,
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
                // Drain async events from the audio thread (non-blocking).
                if let Some(ref arx) = async_rx {
                    while let Ok(evt) = arx.try_recv() {
                        let _ = tx.send(Event::Async(evt));
                    }
                }

                // Poll crossterm on a blocking thread to avoid stalling the
                // tokio runtime. select! lets us break on stop_rx.
                let term_event = tokio::select! {
                    _ = &mut stop_rx => break,
                    result = tokio::task::spawn_blocking(move || {
                        if event::poll(tick_rate).unwrap_or(false) {
                            match event::read() {
                                Ok(CrosstermEvent::Key(key)) => TermEvent::Key(key),
                                Ok(CrosstermEvent::Resize(w, h)) => TermEvent::Resize(w, h),
                                _ => TermEvent::Tick,
                            }
                        } else {
                            TermEvent::Tick
                        }
                    }) => {
                        match result {
                            Ok(evt) => evt,
                            Err(_) => break, // JoinError — task panicked
                        }
                    }
                };

                match term_event {
                    TermEvent::Key(key) => { let _ = tx.send(Event::Key(key)); }
                    TermEvent::Resize(w, h) => { let _ = tx.send(Event::Resize(w, h)); }
                    TermEvent::Tick => { let _ = tx.send(Event::Tick); }
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

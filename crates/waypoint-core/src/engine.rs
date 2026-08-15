//! Wires a source to the parser and publishes the resulting state.
//!
//! Frontends hold an [`Engine`], read [`Engine::state`] to render, and wait on
//! [`Engine::updates`] to know when to redraw. State lives behind an `RwLock`
//! rather than being cloned into a channel because the track can hold hundreds
//! of thousands of points — cloning that per fix would dwarf the actual work.

use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

use tokio::sync::{mpsc, watch};

use crate::parser::Aggregator;
use crate::source::{Cancel, FileMode, ReplaySpeed, SourceConfig, SourceEvent, run_source};
use crate::state::{ConnectionStatus, GnssState};
use crate::trip::TripConfig;

/// Bound on the event queue. Deep enough to absorb a burst from a fast replay,
/// shallow enough that a stalled consumer applies backpressure instead of
/// growing without limit.
const EVENT_QUEUE_DEPTH: usize = 4096;

pub struct Engine {
    state: Arc<RwLock<GnssState>>,
    updates_tx: watch::Sender<u64>,
    updates_rx: watch::Receiver<u64>,
    config: TripConfig,
    active: Mutex<Option<ActiveSource>>,
}

struct ActiveSource {
    config: SourceConfig,
    cancel: Cancel,
}

impl Engine {
    /// Must be called from within a Tokio runtime context.
    pub fn new(config: TripConfig) -> Self {
        let (updates_tx, updates_rx) = watch::channel(0);
        Self {
            state: Arc::new(RwLock::new(GnssState::default())),
            updates_tx,
            updates_rx,
            config,
            active: Mutex::new(None),
        }
    }

    /// Borrow the current state for rendering. Held only for the duration of a
    /// frame — the ingest task never holds the write lock across an await.
    pub fn state(&self) -> RwLockReadGuard<'_, GnssState> {
        self.state.read().unwrap_or_else(|err| err.into_inner())
    }

    /// Bumped once per applied event; watch semantics mean a slow frontend sees
    /// one notification rather than a backlog.
    pub fn updates(&self) -> watch::Receiver<u64> {
        self.updates_rx.clone()
    }

    /// Live replay rate of the running source, when it is a paced file replay.
    /// Frontends adjust this to change speed without restarting the source and
    /// losing the accumulated track.
    pub fn replay_speed(&self) -> Option<ReplaySpeed> {
        self.active.lock().ok().and_then(|guard| {
            guard.as_ref().and_then(|active| match &active.config {
                SourceConfig::File {
                    mode: FileMode::Replay { speed },
                    ..
                } => Some(speed.clone()),
                _ => None,
            })
        })
    }

    pub fn current_source(&self) -> Option<SourceConfig> {
        self.active
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|a| a.config.clone()))
    }

    /// Stop any running source, discard the previous session's data, and start
    /// reading from `config`.
    pub fn set_source(&self, config: SourceConfig) {
        self.stop();

        let label = config.label();
        {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            *state = GnssState::new(label);
            state.connection = ConnectionStatus::Connecting;
        }
        self.notify();

        let cancel = Cancel::new();
        let (tx, rx) = mpsc::channel(EVENT_QUEUE_DEPTH);

        tokio::spawn(run_source(config.clone(), tx, cancel.clone()));
        tokio::spawn(ingest(
            rx,
            Arc::clone(&self.state),
            self.updates_tx.clone(),
            self.config,
        ));

        if let Ok(mut active) = self.active.lock() {
            *active = Some(ActiveSource { config, cancel });
        }
    }

    /// Cancel the running source. The accumulated track and statistics are kept.
    pub fn stop(&self) {
        if let Ok(mut active) = self.active.lock()
            && let Some(previous) = active.take()
        {
            previous.cancel.cancel();
        }
    }

    /// Clear track, plots and trip statistics without disturbing the connection.
    pub fn reset_trip(&self) {
        {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            state.reset_trip();
        }
        self.notify();
    }

    fn notify(&self) {
        self.updates_tx.send_modify(|version| *version += 1);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn ingest(
    mut rx: mpsc::Receiver<SourceEvent>,
    state: Arc<RwLock<GnssState>>,
    updates: watch::Sender<u64>,
    config: TripConfig,
) {
    let mut aggregator = Aggregator::new(config);

    while let Some(event) = rx.recv().await {
        {
            let mut state = state.write().unwrap_or_else(|err| err.into_inner());
            match event {
                SourceEvent::Line(line) => aggregator.ingest_line(&mut state, &line),
                SourceEvent::Status(status) => {
                    tracing::info!(status = %status.label(), "source status");
                    state.connection = status;
                }
                SourceEvent::Warning(message) => {
                    tracing::warn!(%message, "source warning");
                    if !state.connection.is_healthy() {
                        state.connection = ConnectionStatus::Failed { message };
                    }
                }
            }
        }
        updates.send_modify(|version| *version += 1);
    }
}

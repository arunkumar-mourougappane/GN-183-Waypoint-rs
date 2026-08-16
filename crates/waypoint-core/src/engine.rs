//! Wires a source to the parser and publishes the resulting state.
//!
//! Frontends hold an [`Engine`], read [`Engine::state`] to render, and wait on
//! [`Engine::updates`] to know when to redraw. State lives behind an `RwLock`
//! rather than being cloned into a channel because the track can hold hundreds
//! of thousands of points — cloning that per fix would dwarf the actual work.

use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

use tokio::sync::{mpsc, watch};

use crate::parser::Aggregator;
use crate::record::{Recorder, RecordingStatus};
use crate::source::{
    Cancel, FileMode, ReplayControl, SourceConfig, SourceControls, SourceEvent, run_source,
};
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
    /// Shared with the ingest task, which offers it every line that arrives.
    recorder: Arc<Mutex<Option<Recorder>>>,
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
            recorder: Arc::new(Mutex::new(None)),
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

    /// What the running source can be asked to do. `None` when nothing is
    /// running, so frontends can hide every transport affordance at once.
    pub fn controls(&self) -> Option<SourceControls> {
        self.active
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|active| active.config.controls()))
    }

    /// Transport handle of the running source, when it is a paced file replay.
    /// Frontends drive rate and pause through this, so neither restarts the
    /// source and loses the accumulated track.
    pub fn replay_control(&self) -> Option<ReplayControl> {
        self.active.lock().ok().and_then(|guard| {
            guard.as_ref().and_then(|active| match &active.config {
                SourceConfig::File {
                    mode: FileMode::Replay { control },
                    ..
                } => Some(control.clone()),
                _ => None,
            })
        })
    }

    /// Re-run the current source from the beginning. Only meaningful for a
    /// recording — a live receiver has no beginning to return to.
    pub fn restart(&self) {
        if let Some(config) = self.current_source()
            && !config.controls().is_live()
        {
            self.set_source(config);
        }
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
            Arc::clone(&self.recorder),
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

    /// Begin capturing the incoming stream to `path`.
    ///
    /// Live sources only. A recording is how a receiver's misbehaviour gets from
    /// the field to a desk, where it can be replayed and scrubbed; a file source
    /// is already that, so capturing one would only copy it.
    pub async fn start_recording(&self, path: impl AsRef<std::path::Path>) -> Result<(), String> {
        match self.controls() {
            Some(SourceControls::Live) => {}
            Some(_) => return Err("only a live source can be recorded".into()),
            None => return Err("no source is running".into()),
        }

        let recorder = Recorder::start(path)
            .await
            .map_err(|err| format!("could not start recording: {err}"))?;
        *self.recorder.lock().unwrap_or_else(|e| e.into_inner()) = Some(recorder);
        self.notify();
        Ok(())
    }

    /// Close the capture. The file is flushed as the recorder is dropped.
    pub fn stop_recording(&self) {
        self.recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        self.notify();
    }

    pub fn recording(&self) -> Option<RecordingStatus> {
        self.recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(Recorder::status)
    }

    /// Whether capturing is possible right now, which is what decides if the
    /// control is offered at all.
    pub fn can_record(&self) -> bool {
        self.controls().is_some_and(SourceControls::is_live)
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
    recorder: Arc<Mutex<Option<Recorder>>>,
) {
    let mut aggregator = Aggregator::new(config);

    while let Some(event) = rx.recv().await {
        {
            let mut state = state.write().unwrap_or_else(|err| err.into_inner());
            match event {
                SourceEvent::Line(line) => {
                    // Captured before parsing, so the file holds what arrived
                    // rather than what could be understood.
                    if let Ok(guard) = recorder.lock()
                        && let Some(recorder) = guard.as_ref()
                    {
                        recorder.write(&line);
                    }
                    aggregator.ingest_line(&mut state, &line);
                }
                SourceEvent::Status(status) => {
                    tracing::info!(status = %status.label(), "source status");
                    state.connection = status;
                }
                SourceEvent::Seeked => {
                    tracing::info!("source seeked; clearing derived state");
                    state.clear_after_seek();
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

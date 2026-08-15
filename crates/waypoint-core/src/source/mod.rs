//! Input transports.
//!
//! Serial, TCP and file differ only in how bytes arrive and what "disconnected"
//! means; everything downstream sees the same [`SourceEvent`] stream.

pub mod file;
pub mod serial;
pub mod tcp;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::state::ConnectionStatus;

/// Cooperative cancellation, shared with blocking reader threads that can't be
/// dropped mid-`await` the way a task can.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone)]
pub enum SourceEvent {
    /// One raw line, checksum not yet validated.
    Line(String),
    Status(ConnectionStatus),
    /// A recoverable problem worth surfacing without tearing the session down.
    Warning(String),
    /// The source jumped to a different point in a recording. Everything derived
    /// from the sentences before it — the track, the trip, the satellites in
    /// view — describes a stretch of log that is no longer where we are.
    Seeked,
}

/// What a running source can be asked to do. Frontends branch on this rather
/// than on the transport, because the meaningful split is not serial-vs-TCP but
/// live-vs-recorded: a receiver cannot be paused or rewound, and a finished
/// recording cannot go stale or reconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceControls {
    /// A receiver producing data in real time. Can drop out and reconnect, can
    /// go quiet, cannot be replayed or retimed.
    Live,
    /// A paced replay of a recording: pausable, retimable, restartable, and with
    /// a known position within a known length.
    Replay,
    /// A log parsed as fast as it can be read. Finishes on its own; there is
    /// nothing to control while it runs.
    Instant,
}

impl SourceControls {
    pub fn is_live(self) -> bool {
        matches!(self, SourceControls::Live)
    }

    /// Whether transport controls (pause, rate, restart) apply.
    pub fn is_replay(self) -> bool {
        matches!(self, SourceControls::Replay)
    }
}

/// Shared transport state for a file replay: rate, pause, and position.
///
/// Held jointly by the frontend and the running source so a replay can be
/// retimed, paused and reported on in place. Restarting the source to do any of
/// that would discard the accumulated track, which is the opposite of what
/// someone studying a log wants.
///
/// The rate is stored as `f32` bit patterns because there is no `AtomicF32`.
#[derive(Debug, Clone, Default)]
pub struct ReplayControl(Arc<ReplayState>);

/// Sentinel for "no seek pending". Requests are stored as parts-per-million of
/// the log so one atomic carries both the request and its absence.
const NO_SEEK: i64 = -1;
const SEEK_SCALE: f64 = 1_000_000.0;

#[derive(Debug)]
struct ReplayState {
    speed_bits: AtomicU32,
    paused: AtomicBool,
    bytes_read: AtomicU64,
    total_bytes: AtomicU64,
    seek_request: AtomicI64,
}

impl Default for ReplayState {
    fn default() -> Self {
        Self {
            speed_bits: AtomicU32::new(1.0f32.to_bits()),
            paused: AtomicBool::new(false),
            bytes_read: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            seek_request: AtomicI64::new(NO_SEEK),
        }
    }
}

impl ReplayControl {
    /// Slowest and fastest usable multipliers. The lower bound keeps a wait from
    /// growing unbounded; the upper is past the point where a terminal or a map
    /// can show anything meaningful anyway.
    pub const MIN_SPEED: f32 = 0.1;
    pub const MAX_SPEED: f32 = 200.0;

    pub fn new(speed: f32) -> Self {
        let control = Self::default();
        control.set_speed(speed);
        control
    }

    pub fn speed(&self) -> f32 {
        f32::from_bits(self.0.speed_bits.load(Ordering::Relaxed))
    }

    pub fn set_speed(&self, speed: f32) {
        self.0.speed_bits.store(
            speed.clamp(Self::MIN_SPEED, Self::MAX_SPEED).to_bits(),
            Ordering::Relaxed,
        );
    }

    /// Step the rate geometrically, which is how it reads to a user: half speed,
    /// double speed, rather than plus or minus one.
    pub fn scale_speed(&self, factor: f32) {
        self.set_speed(self.speed() * factor);
    }

    pub fn is_paused(&self) -> bool {
        self.0.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.0.paused.store(paused, Ordering::Relaxed);
    }

    pub fn toggle_paused(&self) {
        self.set_paused(!self.is_paused());
    }

    /// Fraction of the log consumed, 0.0 to 1.0. `None` until the source has
    /// opened the file and learned its length.
    pub fn progress(&self) -> Option<f32> {
        let total = self.0.total_bytes.load(Ordering::Relaxed);
        (total > 0).then(|| {
            let read = self.0.bytes_read.load(Ordering::Relaxed);
            (read as f32 / total as f32).clamp(0.0, 1.0)
        })
    }

    /// Ask the running replay to jump to a fraction of the log, as dragging a
    /// media player's scrub bar does. Takes effect at the next sentence.
    pub fn seek_to(&self, fraction: f32) {
        let ppm = (fraction.clamp(0.0, 1.0) as f64 * SEEK_SCALE) as i64;
        self.0.seek_request.store(ppm, Ordering::Relaxed);
    }

    /// Move by a fraction of the log relative to where it is now, for keyboard
    /// nudging. Negative rewinds.
    pub fn seek_by(&self, delta: f32) {
        let from = self.progress().unwrap_or(0.0);
        self.seek_to(from + delta);
    }

    /// Whether a seek is waiting to be acted on, without claiming it.
    pub(crate) fn seek_pending(&self) -> bool {
        self.0.seek_request.load(Ordering::Relaxed) != NO_SEEK
    }

    /// Claim a pending seek, if any. Clearing it here means a request is acted
    /// on once, however many frames it took the user to let go of the slider.
    pub(crate) fn take_seek(&self) -> Option<f64> {
        match self.0.seek_request.swap(NO_SEEK, Ordering::Relaxed) {
            NO_SEEK => None,
            ppm => Some((ppm as f64 / SEEK_SCALE).clamp(0.0, 1.0)),
        }
    }

    pub(crate) fn set_total_bytes(&self, total: u64) {
        self.0.total_bytes.store(total, Ordering::Relaxed);
    }

    /// Absolute position, so a seek reports where it landed rather than adding
    /// to a count that no longer means anything.
    pub(crate) fn set_position(&self, bytes: u64) {
        self.0.bytes_read.store(bytes, Ordering::Relaxed);
    }

    /// Rewind the position counter. Called when a replay starts, so restarting a
    /// source reuses the handle without inheriting a stale progress reading or a
    /// seek left over from the previous run.
    pub(crate) fn rewind(&self) {
        self.0.bytes_read.store(0, Ordering::Relaxed);
        self.0.seek_request.store(NO_SEEK, Ordering::Relaxed);
    }

    /// Peg the position to the end of the log.
    ///
    /// Per-line accounting cannot be exact — the reader strips line endings
    /// whose width it does not report, so a CRLF log accumulates a byte short
    /// per line — and a progress bar that stops at 99% reads as a stall. The
    /// source calls this when it reaches EOF.
    pub(crate) fn complete(&self) {
        let total = self.0.total_bytes.load(Ordering::Relaxed);
        self.0.bytes_read.store(total, Ordering::Relaxed);
    }
}

/// Compares observable transport state, so two handles in the same position and
/// rate are equal even when they are different allocations.
impl PartialEq for ReplayControl {
    fn eq(&self, other: &Self) -> bool {
        self.speed() == other.speed() && self.is_paused() == other.is_paused()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileMode {
    /// Parse the whole log as fast as possible.
    Instant,
    /// Pace sentences by their own timestamps, so the UI animates as it would
    /// from a live receiver. The transport is live: rate, pause and position are
    /// all readable and writable while it runs.
    Replay { control: ReplayControl },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceConfig {
    Serial { path: String, baud: u32 },
    Tcp { addr: String },
    File { path: PathBuf, mode: FileMode },
}

impl SourceConfig {
    /// What the frontends may offer for this source.
    pub fn controls(&self) -> SourceControls {
        match self {
            SourceConfig::Serial { .. } | SourceConfig::Tcp { .. } => SourceControls::Live,
            SourceConfig::File {
                mode: FileMode::Replay { .. },
                ..
            } => SourceControls::Replay,
            SourceConfig::File {
                mode: FileMode::Instant,
                ..
            } => SourceControls::Instant,
        }
    }

    /// Human-readable description for the UI's source indicator.
    pub fn label(&self) -> String {
        match self {
            SourceConfig::Serial { path, baud } => format!("serial {path} @ {baud}"),
            SourceConfig::Tcp { addr } => format!("tcp {addr}"),
            SourceConfig::File { path, mode } => {
                let suffix = match mode {
                    FileMode::Instant => String::new(),
                    // Deliberately not the live rate: this label names the
                    // session, and re-rendering it on every speed change would
                    // make the source indicator jitter.
                    FileMode::Replay { .. } => " (replay)".to_string(),
                };
                format!("file {}{suffix}", path.display())
            }
        }
    }
}

/// Run a source until it finishes or is cancelled, pushing events to `tx`.
pub async fn run_source(config: SourceConfig, tx: mpsc::Sender<SourceEvent>, cancel: Cancel) {
    match config {
        SourceConfig::Serial { path, baud } => serial::run(path, baud, tx, cancel).await,
        SourceConfig::Tcp { addr } => tcp::run(addr, tx, cancel).await,
        SourceConfig::File { path, mode } => file::run(path, mode, tx, cancel).await,
    }
}

/// List serial ports available on this machine, for the source picker.
pub fn available_serial_ports() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
}

/// Baud rates worth offering in a picker. GPS modules are almost always one of
/// these — 4800 for older NMEA 0183 devices, 9600 as the common default.
pub const COMMON_BAUD_RATES: [u32; 7] = [4800, 9600, 19200, 38400, 57600, 115200, 230400];

//! Input transports.
//!
//! Serial, TCP and file differ only in how bytes arrive and what "disconnected"
//! means; everything downstream sees the same [`SourceEvent`] stream.

pub mod file;
pub mod serial;
pub mod tcp;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
}

/// Live replay rate, shared between the frontend and the running file source so
/// it can be changed mid-replay without restarting (which would discard the
/// track accumulated so far).
///
/// Stored as the bit pattern of an `f32` because there is no `AtomicF32`.
#[derive(Debug, Clone)]
pub struct ReplaySpeed(Arc<AtomicU32>);

impl ReplaySpeed {
    /// Slowest and fastest usable multipliers. The lower bound keeps a sleep
    /// from growing unbounded; the upper is past the point where a terminal or
    /// a map can show anything meaningful anyway.
    pub const MIN: f32 = 0.1;
    pub const MAX: f32 = 200.0;

    pub fn new(speed: f32) -> Self {
        Self(Arc::new(AtomicU32::new(
            speed.clamp(Self::MIN, Self::MAX).to_bits(),
        )))
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, speed: f32) {
        self.0.store(
            speed.clamp(Self::MIN, Self::MAX).to_bits(),
            Ordering::Relaxed,
        );
    }

    /// Step the rate geometrically, which is how it reads to a user: half speed,
    /// double speed, rather than ±1.
    pub fn scale(&self, factor: f32) {
        self.set(self.get() * factor);
    }
}

impl Default for ReplaySpeed {
    fn default() -> Self {
        Self::new(1.0)
    }
}

/// Compares current rates, so two handles set to the same speed are equal even
/// when they are different allocations.
impl PartialEq for ReplaySpeed {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileMode {
    /// Parse the whole log as fast as possible.
    Instant,
    /// Pace sentences by their own timestamps, so the UI animates as it would
    /// from a live receiver. The multiplier is live: 2.0 is twice real time.
    Replay { speed: ReplaySpeed },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceConfig {
    Serial { path: String, baud: u32 },
    Tcp { addr: String },
    File { path: PathBuf, mode: FileMode },
}

impl SourceConfig {
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

//! Input transports.
//!
//! Serial, TCP and file differ only in how bytes arrive and what "disconnected"
//! means; everything downstream sees the same [`SourceEvent`] stream.

pub mod file;
pub mod serial;
pub mod tcp;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileMode {
    /// Parse the whole log as fast as possible.
    Instant,
    /// Pace sentences by their own timestamps, so the UI animates as it would
    /// from a live receiver. `speed` multiplies real time (2.0 = twice as fast).
    Replay { speed: f32 },
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
                    FileMode::Instant => "".to_string(),
                    FileMode::Replay { speed } => format!(" (replay ×{speed})"),
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

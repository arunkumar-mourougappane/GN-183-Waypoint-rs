//! Transport-agnostic NMEA 0183 ingestion, parsing and GNSS state aggregation.
//!
//! This crate knows nothing about how the data is displayed; both the `egui`
//! and `ratatui` frontends drive it through [`Engine`].
//!
//! ```no_run
//! use waypoint_core::{Engine, SourceConfig, FileMode, ReplayControl, TripConfig};
//!
//! # async fn example() {
//! let engine = Engine::new(TripConfig::default());
//! engine.set_source(SourceConfig::File {
//!     path: "track.nmea".into(),
//!     mode: FileMode::Replay { control: ReplayControl::new(4.0) },
//! });
//!
//! // The transport is live: this retimes and pauses the replay in flight.
//! if let Some(control) = engine.replay_control() {
//!     control.set_speed(10.0);
//!     control.toggle_paused();
//! }
//!
//! let mut updates = engine.updates();
//! while updates.changed().await.is_ok() {
//!     let state = engine.state();
//!     println!("{} sats, fix {}", state.sats_in_view(), state.fix_mode.label());
//! }
//! # }
//! ```

pub mod engine;
pub mod parser;
pub mod record;
pub mod source;
pub mod state;
pub mod trip;

pub use engine::Engine;
pub use parser::{Aggregator, ChecksumResult, verify_checksum};
pub use record::{Recorder, RecordingStatus};
pub use source::{
    COMMON_BAUD_RATES, FileMode, ReplayControl, SourceConfig, SourceControls, SourceEvent,
    available_serial_ports,
};
pub use state::{
    ConnectionStatus, Constellation, FixMode, FixQuality, GnssState, MetricSample, RawSentence,
    STALE_AFTER, SatelliteInfo, SentenceCounters, SentenceOutcome, StatusTone, StreamHealth,
    TrackPoint,
};
pub use trip::{TripConfig, TripStats, haversine_distance_m};

/// Knots to kilometres per hour.
pub const KNOTS_TO_KMH: f32 = 1.852;
/// Knots to miles per hour.
pub const KNOTS_TO_MPH: f32 = 1.150_779;
/// Metres to feet.
pub const M_TO_FT: f32 = 3.280_84;

/// Qualitative reading of an HDOP value, for the quality band shown next to the
/// raw number.
pub fn hdop_rating(hdop: f32) -> &'static str {
    match hdop {
        h if h < 1.0 => "Ideal",
        h if h < 2.0 => "Excellent",
        h if h < 5.0 => "Good",
        h if h < 10.0 => "Moderate",
        h if h < 20.0 => "Fair",
        _ => "Poor",
    }
}

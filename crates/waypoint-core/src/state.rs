//! Aggregated GNSS state: the single view of "what the receiver is doing right now"
//! that both frontends render.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use nmea::sentences::{FixType as NmeaFixType, GnssType};

use crate::trip::TripStats;

/// Cap on retained track points. Trip statistics are accumulated incrementally,
/// so dropping the oldest points never changes the computed totals.
const MAX_TRACK_POINTS: usize = 200_000;
/// Cap on retained metric samples (one per fix epoch; ~1 hour at 1 Hz).
const MAX_HISTORY: usize = 3_600;
/// Cap on the raw sentence ring shown in the debug view.
const MAX_RAW_SENTENCES: usize = 200;

/// 2D/3D fix state, from GSA field 2. This is the canonical "do we have a fix"
/// signal, distinct from GGA's finer-grained [`FixQuality`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FixMode {
    #[default]
    NoFix,
    Fix2D,
    Fix3D,
}

impl FixMode {
    pub fn label(self) -> &'static str {
        match self {
            FixMode::NoFix => "No Fix",
            FixMode::Fix2D => "2D",
            FixMode::Fix3D => "3D",
        }
    }

    pub fn has_fix(self) -> bool {
        !matches!(self, FixMode::NoFix)
    }
}

/// GGA fix quality — how the fix was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FixQuality {
    #[default]
    Invalid,
    Gps,
    DGps,
    Pps,
    Rtk,
    FloatRtk,
    Estimated,
    Manual,
    Simulation,
}

impl FixQuality {
    pub fn label(self) -> &'static str {
        match self {
            FixQuality::Invalid => "Invalid",
            FixQuality::Gps => "GPS",
            FixQuality::DGps => "DGPS",
            FixQuality::Pps => "PPS",
            FixQuality::Rtk => "RTK Fixed",
            FixQuality::FloatRtk => "RTK Float",
            FixQuality::Estimated => "Estimated",
            FixQuality::Manual => "Manual",
            FixQuality::Simulation => "Simulation",
        }
    }

    /// Whether this quality represents a real, trustworthy position solution.
    pub fn is_valid(self) -> bool {
        matches!(
            self,
            FixQuality::Gps
                | FixQuality::DGps
                | FixQuality::Pps
                | FixQuality::Rtk
                | FixQuality::FloatRtk
        )
    }
}

impl From<NmeaFixType> for FixQuality {
    fn from(value: NmeaFixType) -> Self {
        match value {
            NmeaFixType::Invalid => FixQuality::Invalid,
            NmeaFixType::Gps => FixQuality::Gps,
            NmeaFixType::DGps => FixQuality::DGps,
            NmeaFixType::Pps => FixQuality::Pps,
            NmeaFixType::Rtk => FixQuality::Rtk,
            NmeaFixType::FloatRtk => FixQuality::FloatRtk,
            NmeaFixType::Estimated => FixQuality::Estimated,
            NmeaFixType::Manual => FixQuality::Manual,
            NmeaFixType::Simulation => FixQuality::Simulation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Constellation {
    Gps,
    Glonass,
    Galileo,
    Beidou,
    Qzss,
    NavIC,
}

impl Constellation {
    /// Short label for dense UI (satellite tables, per-constellation counts).
    pub fn short(self) -> &'static str {
        match self {
            Constellation::Gps => "GP",
            Constellation::Glonass => "GL",
            Constellation::Galileo => "GA",
            Constellation::Beidou => "BD",
            Constellation::Qzss => "QZ",
            Constellation::NavIC => "IN",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Constellation::Gps => "GPS",
            Constellation::Glonass => "GLONASS",
            Constellation::Galileo => "Galileo",
            Constellation::Beidou => "BeiDou",
            Constellation::Qzss => "QZSS",
            Constellation::NavIC => "NavIC",
        }
    }

    pub const ALL: [Constellation; 6] = [
        Constellation::Gps,
        Constellation::Glonass,
        Constellation::Galileo,
        Constellation::Beidou,
        Constellation::Qzss,
        Constellation::NavIC,
    ];
}

impl From<GnssType> for Constellation {
    fn from(value: GnssType) -> Self {
        match value {
            GnssType::Gps => Constellation::Gps,
            GnssType::Glonass => Constellation::Glonass,
            GnssType::Galileo => Constellation::Galileo,
            GnssType::Beidou => Constellation::Beidou,
            GnssType::Qzss => Constellation::Qzss,
            GnssType::NavIC => Constellation::NavIC,
        }
    }
}

/// One satellite as reported by GSV, plus whether GSA says it's used in the fix.
#[derive(Debug, Clone, PartialEq)]
pub struct SatelliteInfo {
    pub prn: u32,
    pub constellation: Constellation,
    /// Degrees above the horizon, 0-90.
    pub elevation: Option<f32>,
    /// Degrees clockwise from true north, 0-359.
    pub azimuth: Option<f32>,
    /// Carrier-to-noise density, dB-Hz. `None` means in view but not tracked.
    pub snr: Option<f32>,
    pub used_in_fix: bool,
}

/// A single logged position along the track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPoint {
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: Option<f32>,
    pub speed_knots: Option<f32>,
    pub course_deg: Option<f32>,
    pub timestamp: DateTime<Utc>,
}

/// One sample of the plotted parameters, recorded per fix epoch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricSample {
    pub timestamp: DateTime<Utc>,
    pub speed_knots: Option<f32>,
    pub altitude_m: Option<f32>,
    pub hdop: Option<f32>,
    pub sats_used: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionStatus {
    #[default]
    Idle,
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
    },
    Disconnected,
    Failed {
        message: String,
    },
}

impl ConnectionStatus {
    pub fn label(&self) -> String {
        match self {
            ConnectionStatus::Idle => "Idle".to_string(),
            ConnectionStatus::Connecting => "Connecting".to_string(),
            ConnectionStatus::Connected => "Connected".to_string(),
            ConnectionStatus::Reconnecting { attempt } => format!("Reconnecting (#{attempt})"),
            ConnectionStatus::Disconnected => "Disconnected".to_string(),
            ConnectionStatus::Failed { message } => format!("Failed: {message}"),
        }
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self, ConnectionStatus::Connected)
    }
}

/// How long a live source may go quiet before the UI calls it stale. A GNSS
/// receiver emits at least once a second, so three seconds of silence means
/// something is wrong rather than merely slow.
pub const STALE_AFTER: Duration = Duration::from_secs(3);

/// Liveness of the incoming stream: is data still arriving, and how fast.
///
/// Only meaningful for a live receiver. A finished recording is not "stale", it
/// is simply over, which the connection status already says.
#[derive(Debug, Clone, Default)]
pub struct StreamHealth {
    pub last_sentence_at: Option<Instant>,
    /// Sentences per second over the most recent window.
    pub sentences_per_sec: f32,
    window_started_at: Option<Instant>,
    window_count: u32,
}

impl StreamHealth {
    /// Recompute the rate roughly once a second, which is both the cadence a
    /// receiver reports at and often enough for a readout a human is watching.
    fn record_sentence(&mut self) {
        let now = Instant::now();
        self.last_sentence_at = Some(now);

        let started = *self.window_started_at.get_or_insert(now);
        self.window_count += 1;

        let elapsed = now.duration_since(started);
        if elapsed >= Duration::from_secs(1) {
            self.sentences_per_sec = self.window_count as f32 / elapsed.as_secs_f32();
            self.window_started_at = Some(now);
            self.window_count = 0;
        }
    }

    pub fn since_last_sentence(&self) -> Option<Duration> {
        self.last_sentence_at.map(|at| at.elapsed())
    }

    /// Whether the stream has gone quiet for long enough to be worth flagging.
    pub fn is_stale(&self) -> bool {
        self.since_last_sentence()
            .is_some_and(|since| since > STALE_AFTER)
    }
}

/// Stream health counters — this is a debugger, so how many sentences were
/// rejected and why matters as much as the decoded values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SentenceCounters {
    pub total: u64,
    pub decoded: u64,
    pub checksum_failed: u64,
    pub parse_failed: u64,
    pub unsupported: u64,
    /// Per sentence type (e.g. `GGA`), counting successfully decoded sentences.
    pub by_type: BTreeMap<String, u64>,
}

/// A raw line as received, retained for the debug view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSentence {
    pub text: String,
    pub outcome: SentenceOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentenceOutcome {
    Decoded(String),
    ChecksumFailed,
    Unsupported,
    ParseError(String),
}

/// Accumulates the multi-sentence GSV group for one constellation so the UI
/// never renders a half-reassembled satellite list.
#[derive(Debug, Clone, Default)]
struct GsvAccumulator {
    expected_sentences: u16,
    next_sentence: u16,
    sats: Vec<SatelliteInfo>,
}

#[derive(Debug, Clone, Default)]
pub struct GnssState {
    pub connection: ConnectionStatus,
    pub source_label: String,

    pub fix_mode: FixMode,
    pub fix_quality: FixQuality,
    /// RMC status field: `A` (active) vs `V` (void).
    pub rmc_valid: bool,

    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude_m: Option<f32>,
    pub geoid_separation_m: Option<f32>,
    /// Speed over ground, knots (as reported by RMC/VTG).
    pub speed_knots: Option<f32>,
    /// Course over ground, degrees true.
    pub course_deg: Option<f32>,
    /// True heading from HDT, when the receiver provides it. Distinct from course.
    pub heading_deg: Option<f32>,

    pub hdop: Option<f32>,
    pub vdop: Option<f32>,
    pub pdop: Option<f32>,

    /// Satellites used in the fix, per GGA.
    pub sats_used: Option<u32>,
    /// Satellites in view, reassembled from GSV groups across constellations.
    pub satellites: Vec<SatelliteInfo>,

    pub fix_time: Option<NaiveTime>,
    pub fix_date: Option<NaiveDate>,

    /// Liveness of the incoming stream, for live sources.
    pub health: StreamHealth,
    /// When this session began, for time-to-first-fix.
    pub session_started_at: Option<Instant>,
    /// Wall-clock time from starting the source to the first valid fix — the
    /// standard measure of how long a receiver took to acquire.
    pub time_to_first_fix: Option<Duration>,

    pub track: VecDeque<TrackPoint>,
    pub history: VecDeque<MetricSample>,
    pub trip: TripStats,
    pub counters: SentenceCounters,
    pub raw: VecDeque<RawSentence>,

    // Reassembly bookkeeping, not for rendering.
    gsv_partial: HashMap<Constellation, GsvAccumulator>,
    used_prns: HashSet<u32>,
    gsa_talkers_seen: HashSet<String>,
}

impl GnssState {
    pub fn new(source_label: String) -> Self {
        Self {
            source_label,
            session_started_at: Some(Instant::now()),
            ..Default::default()
        }
    }

    /// Best available timestamp for a fix: the receiver's own UTC when we have
    /// both date and time (so replayed logs keep their original timing), else
    /// wall clock.
    pub fn fix_timestamp(&self) -> DateTime<Utc> {
        match (self.fix_date, self.fix_time) {
            (Some(date), Some(time)) => NaiveDateTime::new(date, time).and_utc(),
            _ => Utc::now(),
        }
    }

    pub fn position(&self) -> Option<(f64, f64)> {
        match (self.latitude, self.longitude) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        }
    }

    /// Satellites in view grouped by constellation, for per-constellation counts.
    pub fn satellites_by_constellation(&self) -> BTreeMap<Constellation, Vec<&SatelliteInfo>> {
        let mut out: BTreeMap<Constellation, Vec<&SatelliteInfo>> = BTreeMap::new();
        for sat in &self.satellites {
            out.entry(sat.constellation).or_default().push(sat);
        }
        out
    }

    pub fn sats_in_view(&self) -> usize {
        self.satellites.len()
    }

    pub(crate) fn record_raw(&mut self, text: String, outcome: SentenceOutcome) {
        self.counters.total += 1;
        // Counts every line off the wire, valid or not: a receiver spewing
        // corrupt sentences is still alive, and that distinction matters when
        // diagnosing a bad cable or the wrong baud rate.
        self.health.record_sentence();
        match &outcome {
            SentenceOutcome::Decoded(kind) => {
                self.counters.decoded += 1;
                *self.counters.by_type.entry(kind.clone()).or_insert(0) += 1;
            }
            SentenceOutcome::ChecksumFailed => self.counters.checksum_failed += 1,
            SentenceOutcome::Unsupported => self.counters.unsupported += 1,
            SentenceOutcome::ParseError(_) => self.counters.parse_failed += 1,
        }
        if self.raw.len() == MAX_RAW_SENTENCES {
            self.raw.pop_front();
        }
        self.raw.push_back(RawSentence { text, outcome });
    }

    pub(crate) fn push_track_point(&mut self, point: TrackPoint) {
        if self.track.len() == MAX_TRACK_POINTS {
            self.track.pop_front();
        }
        self.track.push_back(point);
    }

    pub(crate) fn push_metric_sample(&mut self, sample: MetricSample) {
        if self.history.len() == MAX_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }

    /// Fold one GSV sentence into the per-constellation accumulator, committing
    /// the constellation's satellite list only once the group is complete.
    pub(crate) fn apply_gsv(
        &mut self,
        constellation: Constellation,
        number_of_sentences: u16,
        sentence_num: u16,
        sats: Vec<SatelliteInfo>,
    ) {
        let acc = self.gsv_partial.entry(constellation).or_default();

        // A group restarts at sentence 1; anything out of order means we missed a
        // sentence, so drop the partial group rather than commit a truncated list.
        if sentence_num == 1 {
            acc.sats.clear();
            acc.expected_sentences = number_of_sentences;
            acc.next_sentence = 1;
        } else if acc.next_sentence != sentence_num || acc.expected_sentences != number_of_sentences
        {
            acc.sats.clear();
            acc.next_sentence = 0;
            return;
        }

        acc.sats.extend(sats);
        acc.next_sentence = sentence_num + 1;

        if sentence_num == number_of_sentences {
            let mut complete = std::mem::take(&mut acc.sats);
            acc.next_sentence = 0;
            for sat in &mut complete {
                sat.used_in_fix = self.used_prns.contains(&sat.prn);
            }
            self.satellites.retain(|s| s.constellation != constellation);
            self.satellites.extend(complete);
            self.satellites.sort_by_key(|a| (a.constellation, a.prn));
        }
    }

    /// Fold one GSA sentence's used-PRN list in.
    ///
    /// Multi-constellation receivers emit several GSA sentences per cycle, and
    /// how a new cycle is detected depends on the talker. A `GN` (or `PQ`) talker
    /// emits one GSA *per constellation* under the same talker ID, so a repeated
    /// talker there is normal and must accumulate — only [`Self::begin_epoch`]
    /// ends such a cycle. For per-constellation talkers (`GP`, `GL`, …) a
    /// repeated ID does mean the cycle restarted.
    pub(crate) fn apply_gsa_prns(&mut self, talker_id: &str, prns: Vec<u32>) {
        let accumulates = matches!(talker_id, "GN" | "PQ");
        if !self.gsa_talkers_seen.insert(talker_id.to_string()) && !accumulates {
            self.gsa_talkers_seen.clear();
            self.used_prns.clear();
            self.gsa_talkers_seen.insert(talker_id.to_string());
        }
        self.used_prns.extend(prns);

        let used = &self.used_prns;
        for sat in &mut self.satellites {
            sat.used_in_fix = used.contains(&sat.prn);
        }
    }

    /// Mark the start of a fix epoch (GGA). The used-PRN set is rebuilt from the
    /// GSA sentences that follow, which is the only reliable cycle boundary for
    /// receivers that emit every GSA under the `GN` talker.
    pub(crate) fn begin_epoch(&mut self) {
        self.gsa_talkers_seen.clear();
        self.used_prns.clear();
    }

    /// Record how long acquisition took, the first time a valid fix arrives.
    pub(crate) fn note_first_fix(&mut self) {
        if self.time_to_first_fix.is_none()
            && let Some(started) = self.session_started_at
        {
            self.time_to_first_fix = Some(started.elapsed());
        }
    }

    /// Clear the track, plots and trip statistics without touching the live
    /// connection or the current fix values.
    pub fn reset_trip(&mut self) {
        self.track.clear();
        self.history.clear();
        self.trip = TripStats::default();
    }
}

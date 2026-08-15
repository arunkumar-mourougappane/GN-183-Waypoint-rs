//! Turns raw NMEA lines into updates against [`GnssState`].
//!
//! Checksum validation happens here, once, for every source — serial, TCP and
//! file all funnel through [`Aggregator::ingest_line`].

use nmea::sentences::gsa::GsaMode2;
use nmea::sentences::rmc::RmcStatusOfFix;
use nmea::{ParseResult, parse_str};

use crate::state::{
    Constellation, FixMode, GnssState, MetricSample, SatelliteInfo, SentenceOutcome, TrackPoint,
};
use crate::trip::TripConfig;

/// Result of validating a line's `*HH` checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumResult {
    Valid,
    Mismatch {
        expected: u8,
        computed: u8,
    },
    /// No `*HH` suffix at all. Some receivers omit it; the sentence is still
    /// parseable, just unverified.
    Absent,
}

/// Validate the XOR checksum of a raw NMEA line.
///
/// The checksum covers every byte between the leading `$`/`!` and the `*`.
pub fn verify_checksum(line: &str) -> ChecksumResult {
    let body = line
        .trim_start_matches(['$', '!'])
        .trim_end_matches(['\r', '\n']);

    let Some((payload, checksum)) = body.rsplit_once('*') else {
        return ChecksumResult::Absent;
    };

    let Ok(expected) = u8::from_str_radix(checksum.trim(), 16) else {
        return ChecksumResult::Mismatch {
            expected: 0,
            computed: payload.bytes().fold(0u8, |acc, b| acc ^ b),
        };
    };

    let computed = payload.bytes().fold(0u8, |acc, b| acc ^ b);
    if computed == expected {
        ChecksumResult::Valid
    } else {
        ChecksumResult::Mismatch { expected, computed }
    }
}

/// Applies decoded sentences to a [`GnssState`], handling the cross-sentence
/// bookkeeping the raw parser doesn't do: GSV group reassembly, GSA cycle
/// boundaries, and epoch-driven track/plot/trip updates.
#[derive(Debug, Default)]
pub struct Aggregator {
    pub config: TripConfig,
}

impl Aggregator {
    pub fn new(config: TripConfig) -> Self {
        Self { config }
    }

    /// Ingest one raw line. Blank lines are ignored; everything else is counted,
    /// validated, parsed, and recorded in the raw ring for the debug view.
    pub fn ingest_line(&mut self, state: &mut GnssState, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }

        if let ChecksumResult::Mismatch { expected, computed } = verify_checksum(line) {
            tracing::debug!(%line, expected, computed, "checksum mismatch");
            state.record_raw(line.to_string(), SentenceOutcome::ChecksumFailed);
            return;
        }

        match parse_str(line) {
            Ok(ParseResult::Unsupported(sentence_type)) => {
                state.record_raw(line.to_string(), SentenceOutcome::Unsupported);
                tracing::trace!(?sentence_type, "unsupported sentence");
            }
            Ok(parsed) => {
                let kind = format!("{:?}", nmea::SentenceType::from(&parsed));
                state.record_raw(line.to_string(), SentenceOutcome::Decoded(kind));
                self.apply(state, parsed);
            }
            Err(err) => {
                let message = err.to_string();
                tracing::debug!(%line, %message, "parse error");
                state.record_raw(line.to_string(), SentenceOutcome::ParseError(message));
            }
        }
    }

    fn apply(&mut self, state: &mut GnssState, parsed: ParseResult) {
        match parsed {
            // GGA carries the fix quality, altitude, HDOP and satellites-used, and
            // marks the epoch at which the track and plots advance.
            ParseResult::GGA(gga) => {
                state.begin_epoch();
                if let Some(fix_type) = gga.fix_type {
                    state.fix_quality = fix_type.into();
                }
                state.fix_time = gga.fix_time.or(state.fix_time);
                state.latitude = gga.latitude.or(state.latitude);
                state.longitude = gga.longitude.or(state.longitude);
                state.altitude_m = gga.altitude.or(state.altitude_m);
                state.geoid_separation_m = gga.geoid_separation.or(state.geoid_separation_m);
                state.hdop = gga.hdop.or(state.hdop);
                state.sats_used = gga.fix_satellites.or(state.sats_used);

                self.commit_epoch(state);
            }

            // RMC is the primary source of speed and course, and the only sentence
            // carrying the date needed for an absolute timestamp.
            ParseResult::RMC(rmc) => {
                state.rmc_valid = rmc.status_of_fix != RmcStatusOfFix::Invalid;
                state.fix_time = rmc.fix_time.or(state.fix_time);
                state.fix_date = rmc.fix_date.or(state.fix_date);
                if state.rmc_valid {
                    state.latitude = rmc.lat.or(state.latitude);
                    state.longitude = rmc.lon.or(state.longitude);
                    state.speed_knots = rmc.speed_over_ground.or(state.speed_knots);
                    state.course_deg = rmc.true_course.or(state.course_deg);
                }
            }

            // GSA gives the 2D/3D fix mode, the DOP triple, and which PRNs are
            // actually used in the solution.
            ParseResult::GSA(gsa) => {
                state.fix_mode = match gsa.mode2 {
                    GsaMode2::NoFix => FixMode::NoFix,
                    GsaMode2::Fix2D => FixMode::Fix2D,
                    GsaMode2::Fix3D => FixMode::Fix3D,
                };
                state.pdop = gsa.pdop.or(state.pdop);
                state.hdop = gsa.hdop.or(state.hdop);
                state.vdop = gsa.vdop.or(state.vdop);
                state.apply_gsa_prns(
                    gsa.talker_id.as_str(),
                    gsa.fix_sats_prn.iter().copied().collect(),
                );
            }

            ParseResult::GSV(gsv) => {
                let constellation = Constellation::from(gsv.gnss_type);
                let sats = gsv
                    .sats_info
                    .iter()
                    .flatten()
                    .map(|sat| SatelliteInfo {
                        prn: sat.prn(),
                        constellation,
                        elevation: sat.elevation(),
                        azimuth: sat.azimuth(),
                        snr: sat.snr(),
                        used_in_fix: false,
                    })
                    .collect();
                state.apply_gsv(
                    constellation,
                    gsv.number_of_sentences,
                    gsv.sentence_num,
                    sats,
                );
            }

            // VTG duplicates RMC's speed/course on most receivers; useful as a
            // fallback for streams that omit RMC.
            ParseResult::VTG(vtg) => {
                state.speed_knots = vtg.speed_over_ground.or(state.speed_knots);
                state.course_deg = vtg.true_course.or(state.course_deg);
            }

            ParseResult::GLL(gll) => {
                if gll.valid {
                    state.latitude = gll.latitude.or(state.latitude);
                    state.longitude = gll.longitude.or(state.longitude);
                    state.fix_time = gll.fix_time.or(state.fix_time);
                }
            }

            // True heading, when the receiver has a dual-antenna or compass input.
            ParseResult::HDT(hdt) => {
                state.heading_deg = hdt.heading.or(state.heading_deg);
            }

            ParseResult::ZDA(zda) => {
                state.fix_time = zda.utc_time.or(state.fix_time);
                if let Some(date) = zda.utc_date() {
                    state.fix_date = Some(date);
                }
            }

            _ => {}
        }
    }

    /// Advance the track, plot history and trip statistics. Called once per fix
    /// epoch (on GGA), so all sentences of the epoch have been folded in first.
    fn commit_epoch(&mut self, state: &mut GnssState) {
        let timestamp = state.fix_timestamp();

        state.push_metric_sample(MetricSample {
            timestamp,
            speed_knots: state.speed_knots,
            altitude_m: state.altitude_m,
            hdop: state.hdop,
            sats_used: state.sats_used,
        });
        state.trip.update_quality(state.hdop, state.sats_used);

        // Only a real fix moves the track and the trip totals. Without a valid
        // fix the receiver still reports a stale position, which would otherwise
        // register as a jump back and forth.
        if !state.fix_quality.is_valid() || !state.fix_mode.has_fix() {
            return;
        }
        let Some((lat, lon)) = state.position() else {
            return;
        };
        state.note_first_fix();

        let point = TrackPoint {
            lat,
            lon,
            altitude_m: state.altitude_m,
            speed_knots: state.speed_knots,
            course_deg: state.course_deg,
            timestamp,
        };
        state.push_track_point(point);
        let config = self.config;
        state.trip.update(point, &config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GGA: &str = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";

    #[test]
    fn valid_checksum_accepted() {
        assert_eq!(verify_checksum(GGA), ChecksumResult::Valid);
    }

    #[test]
    fn corrupted_payload_detected() {
        let corrupted = GGA.replace("545.4", "545.5");
        assert!(matches!(
            verify_checksum(&corrupted),
            ChecksumResult::Mismatch { .. }
        ));
    }

    #[test]
    fn missing_checksum_reported_as_absent() {
        assert_eq!(
            verify_checksum("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,"),
            ChecksumResult::Absent
        );
    }

    #[test]
    fn gga_updates_position_and_quality() {
        let mut state = GnssState::new("test".into());
        let mut agg = Aggregator::default();

        agg.ingest_line(&mut state, GGA);

        assert_eq!(state.counters.decoded, 1);
        assert_eq!(state.sats_used, Some(8));
        assert_eq!(state.hdop, Some(0.9));
        assert!((state.latitude.unwrap() - 48.1173).abs() < 0.001);
        assert!((state.altitude_m.unwrap() - 545.4).abs() < 0.01);
    }

    #[test]
    fn checksum_failure_is_counted_not_parsed() {
        let mut state = GnssState::new("test".into());
        let mut agg = Aggregator::default();

        agg.ingest_line(&mut state, &GGA.replace("545.4", "545.5"));

        assert_eq!(state.counters.checksum_failed, 1);
        assert_eq!(state.counters.decoded, 0);
        assert_eq!(state.latitude, None);
    }

    #[test]
    fn gsa_sets_fix_mode_and_used_prns() {
        let mut state = GnssState::new("test".into());
        let mut agg = Aggregator::default();

        agg.ingest_line(
            &mut state,
            "$GPGSA,A,3,04,05,,09,12,,,24,,,,,2.5,1.3,2.1*39",
        );

        assert_eq!(state.fix_mode, FixMode::Fix3D);
        assert_eq!(state.hdop, Some(1.3));
        assert_eq!(state.pdop, Some(2.5));
        assert_eq!(state.vdop, Some(2.1));
    }

    #[test]
    fn gsv_group_commits_only_when_complete() {
        let mut state = GnssState::new("test".into());
        let mut agg = Aggregator::default();

        agg.ingest_line(
            &mut state,
            "$GPGSV,2,1,08,01,40,083,46,02,17,308,41,12,07,344,39,14,22,228,45*75",
        );
        // Partial group: nothing visible yet.
        assert_eq!(state.sats_in_view(), 0);

        agg.ingest_line(
            &mut state,
            "$GPGSV,2,2,08,15,58,190,44,17,21,238,40,19,08,033,39,24,54,047,44*76",
        );
        assert_eq!(state.sats_in_view(), 8);
        assert_eq!(state.satellites[0].constellation, Constellation::Gps);
        assert_eq!(state.satellites[0].elevation, Some(40.0));
    }

    /// Receivers that report every GSA under the `GN` talker emit one per
    /// constellation; treating the repeat as a new cycle would leave only the
    /// last constellation's satellites marked as used.
    #[test]
    fn multiple_gn_gsa_sentences_accumulate_used_satellites() {
        let mut state = GnssState::new("test".into());
        let mut agg = Aggregator::default();

        // Two GSV groups so both constellations are in view.
        for line in [
            "$GPGSV,1,1,04,02,55,120,38,05,31,210,32,06,18,066,26,12,74,301,46*79",
            "$GLGSV,1,1,02,66,38,140,35,72,55,222,42*6E",
            "$GNGSA,A,3,02,05,06,12,,,,,,,,,1.7,0.8,1.4,1*38",
            "$GNGSA,A,3,66,72,,,,,,,,,,,1.7,0.8,1.4,2*3C",
        ] {
            agg.ingest_line(&mut state, line);
        }

        let used: Vec<u32> = state
            .satellites
            .iter()
            .filter(|s| s.used_in_fix)
            .map(|s| s.prn)
            .collect();
        assert_eq!(
            used,
            vec![2, 5, 6, 12, 66, 72],
            "both constellations must stay marked"
        );
    }

    #[test]
    fn track_only_advances_on_a_valid_fix() {
        let mut state = GnssState::new("test".into());
        let mut agg = Aggregator::default();

        // GGA alone: no GSA yet, so fix mode is still NoFix.
        agg.ingest_line(&mut state, GGA);
        assert_eq!(state.track.len(), 0);
        assert_eq!(state.history.len(), 1);

        agg.ingest_line(
            &mut state,
            "$GPGSA,A,3,04,05,,09,12,,,24,,,,,2.5,1.3,2.1*39",
        );
        agg.ingest_line(&mut state, GGA);
        assert_eq!(state.track.len(), 1);
    }
}

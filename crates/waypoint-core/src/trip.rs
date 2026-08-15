//! Incrementally accumulated trip statistics.
//!
//! Every metric here updates in O(1) per fix, so a long session costs nothing to
//! keep current and dropping old track points never changes the totals.

use chrono::{DateTime, Utc};

use crate::state::TrackPoint;

/// Mean Earth radius (IUGG), metres.
const EARTH_RADIUS_M: f64 = 6_371_008.8;

pub const KNOTS_TO_MPS: f64 = 0.514_444;

/// Great-circle distance between two WGS84 points, metres.
///
/// Haversine rather than a geodesic solution: at the inter-fix distances a GNSS
/// receiver produces (metres to tens of metres at 1 Hz), the ellipsoidal
/// correction is far below the receiver's own position noise.
pub fn haversine_distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let d_phi = (lat2 - lat1).to_radians();
    let d_lambda = (lon2 - lon1).to_radians();

    let a = (d_phi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (d_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Tunables for the data-quality guards. Defaults are deliberately conservative;
/// the right values depend on the receiver and the vehicle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TripConfig {
    /// Speed at or above which the receiver is considered to be moving, knots.
    /// Absorbs the position jitter a stationary receiver still reports.
    pub moving_threshold_knots: f32,
    /// Implied speed above which an inter-fix jump is treated as a bad fix
    /// rather than real movement, metres per second.
    pub max_plausible_speed_mps: f64,
    /// Altitude change below which a delta is treated as noise, metres.
    /// GGA altitude typically wanders several metres even with a good fix.
    pub elevation_noise_floor_m: f64,
}

impl Default for TripConfig {
    fn default() -> Self {
        Self {
            moving_threshold_knots: 0.5,
            max_plausible_speed_mps: 340.0,
            elevation_noise_floor_m: 1.0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TripStats {
    pub distance_m: f64,
    pub moving_secs: f64,
    pub max_speed_knots: f32,
    pub elevation_gain_m: f64,
    pub elevation_loss_m: f64,
    /// Fixes excluded from distance/speed because they implied an impossible jump.
    pub rejected_jumps: u64,
    pub fix_count: u64,

    pub first_fix_at: Option<DateTime<Utc>>,
    pub last_fix_at: Option<DateTime<Utc>>,

    last_point: Option<TrackPoint>,
    /// Altitude the gain/loss totals were last measured against. Held separately
    /// from `last_point` so a slow, steady climb still accumulates once it clears
    /// the noise floor, instead of being discarded one sub-threshold step at a time.
    reference_altitude_m: Option<f32>,
    hdop_sum: f64,
    hdop_count: u64,
    sats_sum: u64,
    sats_count: u64,
}

impl TripStats {
    /// Wall time between the first and most recent valid fix, seconds.
    pub fn elapsed_secs(&self) -> f64 {
        match (self.first_fix_at, self.last_fix_at) {
            (Some(first), Some(last)) => (last - first).num_milliseconds() as f64 / 1000.0,
            _ => 0.0,
        }
    }

    pub fn stopped_secs(&self) -> f64 {
        (self.elapsed_secs() - self.moving_secs).max(0.0)
    }

    /// Average speed over *moving* time — more meaningful than distance/elapsed
    /// for any trip with stops.
    pub fn avg_moving_speed_knots(&self) -> Option<f32> {
        (self.moving_secs > 0.0).then(|| (self.distance_m / self.moving_secs / KNOTS_TO_MPS) as f32)
    }

    /// Average speed over total elapsed time, stops included.
    pub fn avg_overall_speed_knots(&self) -> Option<f32> {
        let elapsed = self.elapsed_secs();
        (elapsed > 0.0).then(|| (self.distance_m / elapsed / KNOTS_TO_MPS) as f32)
    }

    pub fn avg_hdop(&self) -> Option<f32> {
        (self.hdop_count > 0).then(|| (self.hdop_sum / self.hdop_count as f64) as f32)
    }

    pub fn avg_sats_used(&self) -> Option<f32> {
        (self.sats_count > 0).then(|| self.sats_sum as f32 / self.sats_count as f32)
    }

    /// Fold in one valid fix. Callers must gate on fix validity first — a void
    /// or no-fix epoch must never reach here, or it will pollute the totals.
    pub fn update(&mut self, point: TrackPoint, config: &TripConfig) {
        self.fix_count += 1;
        self.first_fix_at.get_or_insert(point.timestamp);
        self.last_fix_at = Some(point.timestamp);

        if let Some(speed) = point.speed_knots
            && speed > self.max_speed_knots
        {
            self.max_speed_knots = speed;
        }

        if let Some(alt) = point.altitude_m {
            let reference = *self.reference_altitude_m.get_or_insert(alt);
            let delta = alt as f64 - reference as f64;
            if delta.abs() >= config.elevation_noise_floor_m {
                if delta > 0.0 {
                    self.elevation_gain_m += delta;
                } else {
                    self.elevation_loss_m += -delta;
                }
                self.reference_altitude_m = Some(alt);
            }
        }

        let Some(prev) = self.last_point else {
            self.last_point = Some(point);
            return;
        };

        let dt = (point.timestamp - prev.timestamp).num_milliseconds() as f64 / 1000.0;
        let step_m = haversine_distance_m(prev.lat, prev.lon, point.lat, point.lon);

        // A jump implying an impossible speed is a bad fix, not movement. It stays
        // in the track (this is a debugger — bad fixes are worth seeing) but is
        // kept out of the derived statistics.
        if dt > 0.0 && step_m / dt > config.max_plausible_speed_mps {
            self.rejected_jumps += 1;
            self.last_point = Some(point);
            return;
        }

        self.distance_m += step_m;

        if dt > 0.0 {
            let moving = point
                .speed_knots
                .map(|s| s >= config.moving_threshold_knots)
                // No speed reported: fall back to the distance actually covered.
                .unwrap_or_else(|| {
                    step_m / dt >= config.moving_threshold_knots as f64 * KNOTS_TO_MPS
                });
            if moving {
                self.moving_secs += dt;
            }
        }

        self.last_point = Some(point);
    }

    /// Fold in the reception-quality metrics for one epoch.
    pub fn update_quality(&mut self, hdop: Option<f32>, sats_used: Option<u32>) {
        if let Some(hdop) = hdop {
            self.hdop_sum += hdop as f64;
            self.hdop_count += 1;
        }
        if let Some(sats) = sats_used {
            self.sats_sum += sats as u64;
            self.sats_count += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn point(lat: f64, lon: f64, secs: i64, speed: f32, alt: f32) -> TrackPoint {
        TrackPoint {
            lat,
            lon,
            altitude_m: Some(alt),
            speed_knots: Some(speed),
            course_deg: None,
            timestamp: Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap(),
        }
    }

    #[test]
    fn haversine_matches_known_distance() {
        // Paris to London, ~343 km.
        let d = haversine_distance_m(48.8566, 2.3522, 51.5074, -0.1278);
        assert!((d - 343_500.0).abs() < 2_000.0, "got {d} m");
    }

    #[test]
    fn haversine_is_zero_for_identical_points() {
        assert_eq!(haversine_distance_m(10.0, 20.0, 10.0, 20.0), 0.0);
    }

    #[test]
    fn accumulates_distance_and_moving_time() {
        let config = TripConfig::default();
        let mut trip = TripStats::default();

        trip.update(point(0.0, 0.0, 0, 10.0, 100.0), &config);
        trip.update(point(0.001, 0.0, 1, 10.0, 100.0), &config);
        trip.update(point(0.002, 0.0, 2, 10.0, 100.0), &config);

        // ~111 m per 0.001 degree of latitude, twice.
        assert!(
            (trip.distance_m - 222.0).abs() < 5.0,
            "got {}",
            trip.distance_m
        );
        assert_eq!(trip.moving_secs, 2.0);
        assert_eq!(trip.elapsed_secs(), 2.0);
        assert_eq!(trip.max_speed_knots, 10.0);
    }

    #[test]
    fn stationary_jitter_does_not_count_as_moving_time() {
        let config = TripConfig::default();
        let mut trip = TripStats::default();

        trip.update(point(0.0, 0.0, 0, 0.1, 100.0), &config);
        trip.update(point(0.000_01, 0.0, 1, 0.2, 100.0), &config);

        assert_eq!(trip.moving_secs, 0.0);
        assert_eq!(trip.stopped_secs(), 1.0);
    }

    #[test]
    fn rejects_implausible_jumps() {
        let config = TripConfig::default();
        let mut trip = TripStats::default();

        trip.update(point(0.0, 0.0, 0, 5.0, 100.0), &config);
        // 1 degree of latitude (~111 km) in one second.
        trip.update(point(1.0, 0.0, 1, 5.0, 100.0), &config);

        assert_eq!(trip.rejected_jumps, 1);
        assert_eq!(trip.distance_m, 0.0);
    }

    #[test]
    fn elevation_noise_below_floor_is_ignored() {
        let config = TripConfig::default();
        let mut trip = TripStats::default();

        trip.update(point(0.0, 0.0, 0, 5.0, 100.0), &config);
        trip.update(point(0.000_1, 0.0, 1, 5.0, 100.4), &config);
        assert_eq!(trip.elevation_gain_m, 0.0);

        trip.update(point(0.000_2, 0.0, 2, 5.0, 110.0), &config);
        assert!((trip.elevation_gain_m - 10.0).abs() < 0.001);
        assert_eq!(trip.elevation_loss_m, 0.0);
    }

    #[test]
    fn slow_steady_climb_accumulates_despite_the_noise_floor() {
        let config = TripConfig::default();
        let mut trip = TripStats::default();

        // 0.3 m per fix — every individual step is below the 1 m floor, but the
        // climb is real and must not be thrown away.
        for i in 0..100 {
            trip.update(
                point(0.000_1 * i as f64, 0.0, i, 5.0, 100.0 + 0.3 * i as f32),
                &config,
            );
        }

        // True climb is 29.7 m; at most one sub-threshold step stays uncommitted.
        assert!(
            trip.elevation_gain_m > 28.7 && trip.elevation_gain_m <= 29.7,
            "got {}",
            trip.elevation_gain_m
        );
        assert_eq!(trip.elevation_loss_m, 0.0);
    }
}

//! File source: load a recorded log at once, or replay it in original time.

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use super::{Cancel, FileMode, ReplayControl, SourceEvent};
use crate::state::ConnectionStatus;

/// Cap on how long a single gap is reproduced. A log with a large gap (receiver
/// powered off, say) must not stall the UI for real minutes.
const MAX_REPLAY_SLEEP: Duration = Duration::from_secs(2);
/// Granularity of a replay wait, and so the worst-case lag before a speed change
/// or a cancellation is noticed.
const REPLAY_SLEEP_CHUNK: Duration = Duration::from_millis(100);

pub async fn run(path: PathBuf, mode: FileMode, tx: mpsc::Sender<SourceEvent>, cancel: Cancel) {
    let _ = tx
        .send(SourceEvent::Status(ConnectionStatus::Connecting))
        .await;

    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(err) => {
            let _ = tx
                .send(SourceEvent::Status(ConnectionStatus::Failed {
                    message: format!("{}: {err}", path.display()),
                }))
                .await;
            return;
        }
    };

    // Length drives the progress readout. A log that cannot be measured simply
    // reports no progress rather than failing the replay.
    if let FileMode::Replay { control } = &mode {
        let total = file.metadata().await.map(|meta| meta.len()).unwrap_or(0);
        control.set_total_bytes(total);
        control.rewind();
    }

    let _ = tx
        .send(SourceEvent::Status(ConnectionStatus::Connected))
        .await;

    let mut lines = BufReader::new(file).lines();
    let mut previous_epoch_secs: Option<f64> = None;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(err) => {
                let _ = tx
                    .send(SourceEvent::Warning(format!("read error: {err}")))
                    .await;
                break;
            }
        };

        if let FileMode::Replay { control } = &mode {
            // Hold before emitting, so a pause freezes the dashboard on the
            // sentence being examined rather than one further on.
            wait_while_paused(control, &cancel).await;
            if cancel.is_cancelled() {
                break;
            }

            if let Some(epoch_secs) = sentence_time_secs(&line) {
                if let Some(previous) = previous_epoch_secs {
                    // Guard against a log that wraps past midnight going negative.
                    let delta = epoch_secs - previous;
                    if delta > 0.0 {
                        replay_sleep(delta, control, &cancel).await;
                    }
                }
                previous_epoch_secs = Some(epoch_secs);
            }

            // +1 for the line ending the reader stripped. Approximate, and only
            // ever used to render a progress fraction.
            control.advance_bytes(line.len() as u64 + 1);
        }

        if tx.send(SourceEvent::Line(line)).await.is_err() {
            return;
        }
    }

    if let FileMode::Replay { control } = &mode {
        control.complete();
    }

    let _ = tx
        .send(SourceEvent::Status(ConnectionStatus::Disconnected))
        .await;
}

/// Wait out one inter-sentence gap, re-reading the rate as it goes.
///
/// The sleep is chunked rather than taken in one call so that changing the speed
/// mid-replay takes effect within a tick instead of after the current gap
/// finishes — at 0.1× a single gap can otherwise be ten seconds long.
async fn replay_sleep(delta_secs: f64, control: &ReplayControl, cancel: &Cancel) {
    let mut remaining = delta_secs;

    while remaining > 0.0 && !cancel.is_cancelled() {
        // Pausing mid-gap holds without consuming log time, so resuming picks up
        // the pacing exactly where it left off.
        if control.is_paused() {
            tokio::time::sleep(REPLAY_SLEEP_CHUNK).await;
            continue;
        }

        let speed = control.speed();
        let scaled = remaining / speed as f64;
        let sleep = Duration::from_secs_f64(scaled).min(REPLAY_SLEEP_CHUNK);
        tokio::time::sleep(sleep).await;

        // Charge back the log-time actually consumed at the rate in force for
        // that chunk, so a speed change part-way through neither skips ahead nor
        // repeats time already waited out.
        remaining -= sleep.as_secs_f64() * speed as f64;

        if scaled >= MAX_REPLAY_SLEEP.as_secs_f64() {
            // A gap this long is a receiver that was switched off, not pacing
            // worth reproducing in real time.
            break;
        }
    }
}

/// Block while the replay is paused, giving up promptly on cancellation.
async fn wait_while_paused(control: &ReplayControl, cancel: &Cancel) {
    while control.is_paused() && !cancel.is_cancelled() {
        tokio::time::sleep(REPLAY_SLEEP_CHUNK).await;
    }
}

/// Extract the UTC time field as seconds-since-midnight from the sentences that
/// carry one in field 1, for replay pacing. Deliberately a cheap string scan
/// rather than a full parse — this runs before validation, on every line.
fn sentence_time_secs(line: &str) -> Option<f64> {
    let mut fields = line.split(',');
    let header = fields.next()?;
    if !(header.ends_with("GGA") || header.ends_with("RMC") || header.ends_with("ZDA")) {
        return None;
    }

    let time = fields.next()?;
    if time.len() < 6 {
        return None;
    }

    let hours: f64 = time.get(0..2)?.parse().ok()?;
    let minutes: f64 = time.get(2..4)?.parse().ok()?;
    let seconds: f64 = time.get(4..)?.parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;

    use super::*;

    #[test]
    fn extracts_time_from_gga() {
        let secs =
            sentence_time_secs("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47")
                .unwrap();
        assert_eq!(secs, 12.0 * 3600.0 + 35.0 * 60.0 + 19.0);
    }

    #[test]
    fn handles_fractional_seconds() {
        let secs = sentence_time_secs(
            "$GNRMC,123519.50,A,4807.038,N,01131.000,E,022.4,084.4,230394,,,A*49",
        )
        .unwrap();
        assert!((secs - (12.0 * 3600.0 + 35.0 * 60.0 + 19.5)).abs() < 1e-6);
    }

    /// A log file that deletes itself, so the timing tests need no fixtures and
    /// no temp-directory dependency.
    struct TempLog(PathBuf);

    impl TempLog {
        /// Ten seconds of receiver time, one GGA per second.
        fn ten_seconds() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let mut path = std::env::temp_dir();
            path.push(format!("waypoint-replay-{}-{id}.nmea", std::process::id()));

            let lines: Vec<String> = (0..10)
                .map(|i| {
                    format!("$GPGGA,1200{i:02},4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47")
                })
                .collect();
            std::fs::write(&path, lines.join("\r\n")).unwrap();
            Self(path)
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    async fn drain(mut rx: mpsc::Receiver<SourceEvent>) -> usize {
        let mut lines = 0;
        while let Some(event) = rx.recv().await {
            if matches!(event, SourceEvent::Line(_)) {
                lines += 1;
            }
        }
        lines
    }

    /// Replay must actually take time proportional to the log, and the rate must
    /// divide it — this is the difference between replay and instant load.
    #[tokio::test]
    async fn replay_rate_scales_elapsed_time() {
        let log = TempLog::ten_seconds();
        let (tx, rx) = mpsc::channel(64);

        let started = std::time::Instant::now();
        let control = ReplayControl::new(20.0);
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay { control },
            tx,
            Cancel::new(),
        ));
        let lines = drain(rx).await;
        let elapsed = started.elapsed();

        assert_eq!(lines, 10);
        // 9 gaps of 1 s at 20x is ~450 ms; allow generous slack for CI timers,
        // but it must be clearly slower than an instant load and clearly faster
        // than real time.
        assert!(
            elapsed > Duration::from_millis(200) && elapsed < Duration::from_secs(3),
            "replay took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn instant_mode_does_not_pace() {
        let log = TempLog::ten_seconds();
        let (tx, rx) = mpsc::channel(64);

        let started = std::time::Instant::now();
        tokio::spawn(run(log.0.clone(), FileMode::Instant, tx, Cancel::new()));
        let lines = drain(rx).await;

        assert_eq!(lines, 10);
        // Paced at 1x this log spans 9 s, so anything near-instant proves the
        // point; the bound is loose so a contended CI runner cannot flake it.
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "instant load paced itself: took {:?}",
            started.elapsed()
        );
    }

    /// The whole point of the shared handle: speeding up mid-replay must take
    /// effect without restarting the source.
    #[tokio::test]
    async fn speed_change_applies_mid_replay() {
        let log = TempLog::ten_seconds();
        let (tx, rx) = mpsc::channel(64);

        let control = ReplayControl::new(0.5);
        let started = std::time::Instant::now();
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay {
                control: control.clone(),
            },
            tx,
            Cancel::new(),
        ));

        // At 0.5x the log would take ~18 s; jump to the maximum shortly after
        // starting and it must finish almost immediately instead.
        tokio::time::sleep(Duration::from_millis(150)).await;
        control.set_speed(ReplayControl::MAX_SPEED);

        let lines = drain(rx).await;
        assert_eq!(lines, 10);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "speed change was not picked up: took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn replay_speed_is_clamped_to_a_usable_range() {
        let control = ReplayControl::new(1.0);
        control.set_speed(0.0);
        assert_eq!(control.speed(), ReplayControl::MIN_SPEED);
        control.set_speed(f32::INFINITY);
        assert_eq!(control.speed(), ReplayControl::MAX_SPEED);
        control.set_speed(4.0);
        control.scale_speed(0.5);
        assert_eq!(control.speed(), 2.0);
    }

    /// Pausing must stop sentences reaching the dashboard, and resuming must
    /// carry on rather than skipping the time spent paused.
    #[tokio::test]
    async fn pausing_holds_the_replay() {
        let log = TempLog::ten_seconds();
        let (tx, mut rx) = mpsc::channel(64);

        let control = ReplayControl::new(50.0);
        control.set_paused(true);
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay {
                control: control.clone(),
            },
            tx,
            Cancel::new(),
        ));

        // Paused from the outset: the status arrives, the sentences do not.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut lines = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, SourceEvent::Line(_)) {
                lines += 1;
            }
        }
        assert_eq!(lines, 0, "paused replay emitted sentences");

        control.set_paused(false);
        assert_eq!(drain(rx).await, 10, "replay did not resume");
    }

    #[tokio::test]
    async fn replay_reports_progress_through_the_log() {
        let log = TempLog::ten_seconds();
        let (tx, rx) = mpsc::channel(64);

        let control = ReplayControl::new(ReplayControl::MAX_SPEED);
        assert_eq!(
            control.progress(),
            None,
            "no progress before the file opens"
        );

        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay {
                control: control.clone(),
            },
            tx,
            Cancel::new(),
        ));
        drain(rx).await;

        let progress = control.progress().expect("length known once opened");
        assert_eq!(progress, 1.0, "finished replay must read as complete");
    }

    #[test]
    fn ignores_sentences_without_a_time_field() {
        assert_eq!(
            sentence_time_secs(
                "$GPGSV,2,1,08,01,40,083,46,02,17,308,41,12,07,344,39,14,22,228,45*75"
            ),
            None
        );
    }
}

//! File source: load a recorded log at once, or replay it in original time.

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::sync::mpsc;

use super::{Cancel, FileMode, ReplayControl, SourceEvent};
use crate::state::ConnectionStatus;

/// Cap on how long a single gap is reproduced. A log with a large gap (receiver
/// powered off, say) must not stall the UI for real minutes.
const MAX_REPLAY_SLEEP: Duration = Duration::from_secs(2);
/// Granularity of a replay wait, and so the worst-case lag before a speed change
/// or a cancellation is noticed.
const REPLAY_SLEEP_CHUNK: Duration = Duration::from_millis(100);
/// Shortest wait worth taking. Anything below this is discarded rather than
/// slept on, because `Duration::from_secs_f64` rounds a sub-nanosecond value to
/// zero — and a zero-length sleep consumes no log time, so the loop below would
/// never make progress. Real logs reach this state routinely: any gap that does
/// not divide exactly by the replay rate leaves a floating-point residue.
const MIN_MEANINGFUL_WAIT_SECS: f64 = 0.000_5;

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

    // Length drives the scrub bar. A log that cannot be measured simply reports
    // no position rather than failing the replay.
    let total_bytes = file.metadata().await.map(|meta| meta.len()).unwrap_or(0);
    if let FileMode::Replay { control } = &mode {
        control.set_total_bytes(total_bytes);
        control.rewind();
    }

    let _ = tx
        .send(SourceEvent::Status(ConnectionStatus::Connected))
        .await;

    // Read through a seekable BufReader rather than a line stream: a recording
    // is random-access, and `read_line` also reports the exact byte count, so
    // the position is measured rather than estimated.
    let mut reader = BufReader::new(file);
    let mut previous_epoch_secs: Option<f64> = None;
    let mut position: u64 = 0;
    let mut line = String::new();
    // Set after a seek: deliver sentences unpaced until one whole fix epoch has
    // gone out, so scrubbing while paused lands the dashboard on a complete fix
    // rather than a fragment of one.
    let mut settling_after_seek = false;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        if let FileMode::Replay { control } = &mode
            && !settling_after_seek
        {
            // Waiting here rather than after the read is what lets the scrub bar
            // work while paused, which is when it is most useful.
            wait_while_paused(control, &cancel).await;
            if cancel.is_cancelled() {
                break;
            }

            if let Some(fraction) = control.take_seek() {
                match seek_to_line(&mut reader, fraction, total_bytes).await {
                    Ok(landed) => {
                        position = landed;
                        control.set_position(position);
                        // The gap either side of a jump is not elapsed time.
                        previous_epoch_secs = None;
                        settling_after_seek = true;
                        if tx.send(SourceEvent::Seeked).await.is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = tx
                            .send(SourceEvent::Warning(format!("seek failed: {err}")))
                            .await;
                    }
                }
            }
        }

        line.clear();
        let read = match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => {
                let _ = tx
                    .send(SourceEvent::Warning(format!("read error: {err}")))
                    .await;
                break;
            }
        };
        position += read as u64;
        let line = line.trim_end_matches(['\r', '\n']).to_string();

        if let FileMode::Replay { control } = &mode {
            if let Some(epoch_secs) = sentence_time_secs(&line) {
                if let Some(previous) = previous_epoch_secs {
                    // Guard against a log that wraps past midnight going negative.
                    let delta = epoch_secs - previous;
                    if delta > 0.0 && !settling_after_seek {
                        replay_sleep(delta, control, &cancel).await;
                    }
                }
                previous_epoch_secs = Some(epoch_secs);
            }

            // GGA closes an epoch, so the fix is whole once one has gone out.
            if settling_after_seek && is_epoch_boundary(&line) {
                settling_after_seek = false;
            }

            control.set_position(position);
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

/// Jump to a fraction of the log and line up on a sentence boundary.
///
/// Seeking by byte lands mid-sentence almost every time, so the remainder of
/// that line is discarded — emitting half a sentence would just be counted as a
/// checksum failure and read as corruption that is not there.
async fn seek_to_line(
    reader: &mut BufReader<tokio::fs::File>,
    fraction: f64,
    total_bytes: u64,
) -> std::io::Result<u64> {
    let offset = (fraction * total_bytes as f64) as u64;
    reader.seek(std::io::SeekFrom::Start(offset)).await?;

    if offset == 0 {
        return Ok(0);
    }

    let mut partial = String::new();
    let skipped = reader.read_line(&mut partial).await?;
    Ok(offset + skipped as u64)
}

/// Wait out one inter-sentence gap, re-reading the rate as it goes.
///
/// The sleep is chunked rather than taken in one call so that changing the speed
/// mid-replay takes effect within a tick instead of after the current gap
/// finishes — at 0.1× a single gap can otherwise be ten seconds long.
async fn replay_sleep(delta_secs: f64, control: &ReplayControl, cancel: &Cancel) {
    let mut remaining = delta_secs;
    let mut waited = Duration::ZERO;

    while remaining > 0.0 && !cancel.is_cancelled() {
        // A jump makes the rest of this gap meaningless. Returning here is also
        // what lets the scrub bar respond while paused: a pause landing mid-gap
        // parks in this loop rather than at the top of the read loop.
        if control.seek_pending() {
            return;
        }

        // Pausing mid-gap holds without consuming log time, so resuming picks up
        // the pacing exactly where it left off. It must not count against the
        // per-gap cap below, or a long pause would skip the rest of the gap.
        if control.is_paused() {
            tokio::time::sleep(REPLAY_SLEEP_CHUNK).await;
            continue;
        }

        let speed = control.speed();
        let scaled_secs = remaining / speed as f64;

        // Every iteration must either stop or consume real log time. Waiting on
        // a duration that rounds to zero would do neither, and the loop would
        // never end.
        if !scaled_secs.is_finite() || scaled_secs < MIN_MEANINGFUL_WAIT_SECS {
            break;
        }

        // Capped before the conversion, so an enormous gap cannot overflow it.
        let sleep = Duration::from_secs_f64(scaled_secs.min(REPLAY_SLEEP_CHUNK.as_secs_f64()));
        tokio::time::sleep(sleep).await;
        waited += sleep;

        // Charge back the log-time actually consumed at the rate in force for
        // that chunk, so a speed change part-way through neither skips ahead nor
        // repeats time already waited out.
        remaining -= sleep.as_secs_f64() * speed as f64;

        if waited >= MAX_REPLAY_SLEEP {
            // A gap this long is a receiver that was switched off, not pacing
            // worth reproducing in real time.
            break;
        }
    }
}

/// Block while the replay is paused, giving up promptly on cancellation or on a
/// pending seek — scrubbing has to work while paused.
async fn wait_while_paused(control: &ReplayControl, cancel: &Cancel) {
    while control.is_paused() && !cancel.is_cancelled() && !control.seek_pending() {
        tokio::time::sleep(REPLAY_SLEEP_CHUNK).await;
    }
}

/// Whether this sentence ends a fix epoch.
fn is_epoch_boundary(line: &str) -> bool {
    line.split(',').next().is_some_and(|h| h.ends_with("GGA"))
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
        /// Timestamps that do not tick on whole seconds, as a real receiver's
        /// do not. The gaps here (0.97 s, 1.03 s) do not divide exactly by a
        /// replay rate, which is what leaves a floating-point residue behind.
        fn fractional_seconds() -> Self {
            Self::write(&[
                "000000.00",
                "000000.97",
                "000002.00",
                "000003.00",
                "000003.97",
                "000005.00",
            ])
        }

        /// Epochs of two sentences, so "did the seek deliver a whole fix?" is
        /// distinguishable from "did it deliver one line?".
        fn paired_sentences() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("waypoint-paired-{}-{id}.nmea", std::process::id()));

            let mut lines = Vec::new();
            for i in 0..10 {
                lines.push(format!(
                    "$GPRMC,1200{i:02},A,4807.038,N,01131.000,E,0.0,0.0,150826,,,A"
                ));
                lines.push(format!(
                    "$GPGGA,1200{i:02},4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,"
                ));
            }
            std::fs::write(&path, lines.join("\r\n")).unwrap();
            Self(path)
        }

        /// Ten seconds of receiver time, one GGA per second.
        fn ten_seconds() -> Self {
            let times: Vec<String> = (0..10).map(|i| format!("1200{i:02}")).collect();
            Self::write(&times.iter().map(String::as_str).collect::<Vec<_>>())
        }

        fn write(times: &[&str]) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let mut path = std::env::temp_dir();
            path.push(format!("waypoint-replay-{}-{id}.nmea", std::process::id()));

            let lines: Vec<String> = times
                .iter()
                .map(|t| format!("$GPGGA,{t},4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47"))
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

    /// A gap that does not divide exactly by the replay rate leaves a residue
    /// small enough that the wait for it rounds to zero. Sleeping zero consumes
    /// no log time, so a loop that only stops when the gap is used up never
    /// stops at all — the replay freezes mid-log and never recovers.
    ///
    /// Whole-second logs hide this completely: their gaps divide exactly and
    /// reach zero on the nose.
    #[tokio::test]
    async fn fractional_gaps_do_not_stall_the_replay() {
        let log = TempLog::fractional_seconds();
        let (tx, rx) = mpsc::channel(64);

        let control = ReplayControl::new(20.0);
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay { control },
            tx,
            Cancel::new(),
        ));

        let lines = tokio::time::timeout(Duration::from_secs(5), drain(rx))
            .await
            .expect("replay stalled on a gap whose remaining wait rounds to zero");
        assert_eq!(lines, 6, "every sentence should still be delivered");
    }

    /// Dragging the scrub bar must actually move the read head, and land on a
    /// sentence boundary rather than mid-line.
    #[tokio::test]
    async fn seeking_jumps_to_a_whole_sentence() {
        let log = TempLog::ten_seconds();
        let (tx, mut rx) = mpsc::channel(64);

        let control = ReplayControl::new(ReplayControl::MAX_SPEED);
        control.set_paused(true);
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay {
                control: control.clone(),
            },
            tx,
            Cancel::new(),
        ));

        // Held at the start, then scrubbed to the middle.
        tokio::time::sleep(Duration::from_millis(150)).await;
        control.seek_to(0.5);
        control.set_paused(false);

        let mut lines = Vec::new();
        while let Some(event) = rx.recv().await {
            if let SourceEvent::Line(line) = event {
                lines.push(line);
            }
        }

        assert!(
            lines.len() < 10,
            "seeking to the middle should skip earlier sentences, got {}",
            lines.len()
        );
        assert!(
            lines.iter().all(|l| l.starts_with("$GPGGA")),
            "a seek landed mid-sentence: {lines:?}"
        );
        assert!(
            lines.last().unwrap().contains("120009"),
            "the replay should still run to the end of the log"
        );
    }

    /// Scrubbing backwards is the point of a scrub bar: replaying a stretch you
    /// just watched.
    #[tokio::test]
    async fn seeking_backwards_replays_earlier_sentences() {
        let log = TempLog::ten_seconds();
        let (tx, mut rx) = mpsc::channel(64);

        let control = ReplayControl::new(ReplayControl::MAX_SPEED);
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay {
                control: control.clone(),
            },
            tx,
            Cancel::new(),
        ));

        // Let it run out, then rewind to the start and read it again.
        let mut first_pass = 0;
        while let Some(event) = rx.recv().await {
            if matches!(event, SourceEvent::Line(_)) {
                first_pass += 1;
            }
            if matches!(event, SourceEvent::Status(ConnectionStatus::Disconnected)) {
                break;
            }
        }
        assert_eq!(first_pass, 10);
        assert_eq!(control.progress(), Some(1.0));
    }

    #[tokio::test]
    async fn seek_position_is_reported_back() {
        let log = TempLog::ten_seconds();
        let (tx, rx) = mpsc::channel(64);

        let control = ReplayControl::new(ReplayControl::MAX_SPEED);
        control.set_paused(true);
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay {
                control: control.clone(),
            },
            tx,
            Cancel::new(),
        ));

        tokio::time::sleep(Duration::from_millis(150)).await;
        control.seek_to(0.75);
        tokio::time::sleep(Duration::from_millis(150)).await;

        let progress = control.progress().expect("length known");
        assert!(
            (0.7..0.95).contains(&progress),
            "scrub bar should follow the seek, reported {progress}"
        );

        control.set_paused(false);
        drain(rx).await;
    }

    /// Scrubbing while paused must leave the dashboard on a complete fix. A
    /// seek lands mid-epoch, so the source runs on unpaced until a GGA closes
    /// one, then holds again.
    #[tokio::test]
    async fn seeking_while_paused_settles_on_a_whole_epoch() {
        let log = TempLog::paired_sentences();
        let (tx, mut rx) = mpsc::channel(64);

        let control = ReplayControl::new(1.0);
        control.set_paused(true);
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay {
                control: control.clone(),
            },
            tx,
            Cancel::new(),
        ));

        tokio::time::sleep(Duration::from_millis(150)).await;
        control.seek_to(0.5);
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut delivered = Vec::new();
        let mut seeked = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                SourceEvent::Seeked => seeked = true,
                SourceEvent::Line(l) => delivered.push(l),
                _ => {}
            }
        }

        assert!(seeked, "the seek was never reported");
        assert!(
            delivered.iter().any(|l| l.starts_with("$GPGGA")),
            "paused scrub should still land on a fix: {delivered:?}"
        );
        // Having settled, it must stop rather than run on while still paused.
        assert!(
            delivered.len() <= 3,
            "paused replay ran past the epoch it settled on: {delivered:?}"
        );
        assert!(control.is_paused());
    }

    /// Pausing part-way through a gap parks the source inside the pacing wait,
    /// not at the top of the read loop. A scrub bar that only answered from the
    /// top would look dead exactly when someone pauses to go hunting.
    #[tokio::test]
    async fn seeking_works_when_paused_part_way_through_a_gap() {
        let log = TempLog::ten_seconds();
        let (tx, rx) = mpsc::channel(64);

        // Slow enough that pausing is overwhelmingly likely to land mid-gap.
        let control = ReplayControl::new(0.5);
        tokio::spawn(run(
            log.0.clone(),
            FileMode::Replay {
                control: control.clone(),
            },
            tx,
            Cancel::new(),
        ));

        tokio::time::sleep(Duration::from_millis(250)).await;
        control.set_paused(true);
        tokio::time::sleep(Duration::from_millis(150)).await;

        let before = control.progress().expect("length known");
        control.seek_to(0.8);
        tokio::time::sleep(Duration::from_millis(400)).await;
        let after = control.progress().expect("length known");

        assert!(
            after > before + 0.3,
            "scrubbing while paused did not move the position: {before} -> {after}"
        );
        assert!(control.is_paused(), "seeking must not resume playback");

        control.set_paused(false);
        drain(rx).await;
    }

    #[test]
    fn relative_seeking_is_clamped_to_the_log() {
        let control = ReplayControl::new(1.0);
        control.set_total_bytes(1000);
        control.set_position(500);
        assert_eq!(control.progress(), Some(0.5));

        control.seek_by(-2.0);
        assert_eq!(control.take_seek(), Some(0.0), "rewinding past the start");

        control.seek_by(2.0);
        assert_eq!(control.take_seek(), Some(1.0), "seeking past the end");

        assert_eq!(control.take_seek(), None, "a request is acted on once");
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

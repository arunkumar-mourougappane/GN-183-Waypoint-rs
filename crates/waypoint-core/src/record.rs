//! Capturing a live stream to a log the file source can replay.
//!
//! The point of the loop: a receiver misbehaves in the field, you record it, and
//! then you can pause, rewind and scrub over the failure at a desk as many times
//! as it takes. That only makes sense for a live source — "recording" a file
//! would be a file copy.
//!
//! What is written is the raw stream, verbatim, including sentences that failed
//! their checksum. A debugger's recording must not be tidier than what arrived,
//! or it cannot reproduce the fault that made it worth recording.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;

/// Depth of the queue between ingestion and the writer. Deep enough to ride out
/// a slow disk, shallow enough that a stuck one is noticed rather than hidden.
const QUEUE_DEPTH: usize = 4096;
/// How often buffered output reaches the file, so a crash costs about a second
/// of capture rather than everything since the last full buffer.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
struct Counters {
    sentences: AtomicU64,
    bytes: AtomicU64,
    /// Sentences lost because the writer could not keep up. Surfaced rather than
    /// hidden: a recording with a hole in it should say so.
    dropped: AtomicU64,
    failed: AtomicBool,
}

/// What the UI shows about an in-progress capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingStatus {
    pub path: PathBuf,
    pub sentences: u64,
    pub bytes: u64,
    pub dropped: u64,
    /// The file could not be written; the capture is no longer trustworthy.
    pub failed: bool,
}

impl RecordingStatus {
    /// Human-readable size, since a capture's worth is judged by how much of it
    /// there is.
    pub fn size_label(&self) -> String {
        match self.bytes {
            b if b < 1024 => format!("{b} B"),
            b if b < 1024 * 1024 => format!("{:.1} kB", b as f64 / 1024.0),
            b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        }
    }
}

/// Handle to a running capture. Dropping it closes the file.
#[derive(Debug)]
pub struct Recorder {
    tx: mpsc::Sender<String>,
    counters: Arc<Counters>,
    path: PathBuf,
}

impl Recorder {
    /// Begin writing to `path`. Fails immediately if the file cannot be created,
    /// so a bad path is reported when asked for rather than discovered later.
    pub async fn start(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = tokio::fs::File::create(&path).await?;

        let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
        let counters = Arc::new(Counters::default());
        tokio::spawn(write_loop(file, rx, Arc::clone(&counters)));

        Ok(Self { tx, counters, path })
    }

    /// Offer a sentence to the capture. Never blocks: if the writer has fallen
    /// behind, the sentence is counted as dropped rather than stalling the
    /// dashboard, which has a live receiver to keep up with.
    pub fn write(&self, line: &str) {
        if self.tx.try_send(line.to_string()).is_err() {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn status(&self) -> RecordingStatus {
        RecordingStatus {
            path: self.path.clone(),
            sentences: self.counters.sentences.load(Ordering::Relaxed),
            bytes: self.counters.bytes.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            failed: self.counters.failed.load(Ordering::Relaxed),
        }
    }
}

async fn write_loop(
    file: tokio::fs::File,
    mut rx: mpsc::Receiver<String>,
    counters: Arc<Counters>,
) {
    let mut writer = BufWriter::new(file);
    let mut flush = tokio::time::interval(FLUSH_INTERVAL);
    flush.tick().await; // the first tick is immediate

    loop {
        tokio::select! {
            line = rx.recv() => match line {
                Some(line) => {
                    // CRLF, as NMEA 0183 specifies and as the readers strip.
                    // Written back so the file replays as the wire looked.
                    if writer.write_all(line.as_bytes()).await.is_err()
                        || writer.write_all(b"\r\n").await.is_err()
                    {
                        counters.failed.store(true, Ordering::Relaxed);
                        break;
                    }
                    counters.sentences.fetch_add(1, Ordering::Relaxed);
                    counters.bytes.fetch_add(line.len() as u64 + 2, Ordering::Relaxed);
                }
                // The handle was dropped: the capture is over.
                None => break,
            },
            _ = flush.tick() => {
                if writer.flush().await.is_err() {
                    counters.failed.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    }

    if writer.flush().await.is_err() {
        counters.failed.store(true, Ordering::Relaxed);
    }
}

/// A name for a capture nobody wanted to choose: sortable, unambiguous, and
/// obviously ours.
pub fn default_filename(now: chrono::DateTime<chrono::Utc>) -> String {
    format!("waypoint-{}.nmea", now.format("%Y%m%d-%H%M%S"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    struct TempPath(PathBuf);

    impl TempPath {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!("waypoint-rec-{}-{tag}.nmea", std::process::id()));
            Self(path)
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A recording has to be replayable, which means byte-for-byte what arrived
    /// — CRLF included, since that is what the file source expects to strip.
    #[tokio::test]
    async fn captures_sentences_verbatim() {
        let path = TempPath::new("verbatim");
        let recorder = Recorder::start(&path.0).await.unwrap();

        recorder.write("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47");
        recorder.write("$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,,,A*00");
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(recorder);
        tokio::time::sleep(Duration::from_millis(200)).await;

        let written = std::fs::read_to_string(&path.0).unwrap();
        assert!(written.contains("$GPGGA,123519"));
        assert!(written.ends_with("\r\n"), "NMEA lines are CRLF terminated");
        assert_eq!(written.lines().count(), 2);
    }

    /// The corrupt sentence is often the whole reason for recording, so it must
    /// survive into the file.
    #[tokio::test]
    async fn a_damaged_sentence_is_kept() {
        let path = TempPath::new("damaged");
        let recorder = Recorder::start(&path.0).await.unwrap();

        recorder.write("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*00");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = recorder.status();
        drop(recorder);
        tokio::time::sleep(Duration::from_millis(150)).await;

        assert_eq!(status.sentences, 1);
        assert!(
            std::fs::read_to_string(&path.0).unwrap().contains("*00"),
            "a checksum failure must be recorded, not filtered"
        );
    }

    #[tokio::test]
    async fn a_bad_path_fails_when_asked_not_later() {
        let result = Recorder::start("/nonexistent-directory-xyz/capture.nmea").await;
        assert!(result.is_err(), "an unwritable path should refuse up front");
    }

    #[tokio::test]
    async fn status_reports_what_has_been_captured() {
        let path = TempPath::new("status");
        let recorder = Recorder::start(&path.0).await.unwrap();
        assert_eq!(recorder.status().sentences, 0);

        for _ in 0..10 {
            recorder.write("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;

        let status = recorder.status();
        assert_eq!(status.sentences, 10);
        assert!(status.bytes > 600, "got {} bytes", status.bytes);
        assert_eq!(status.dropped, 0);
        assert!(!status.failed);
    }

    #[test]
    fn size_reads_at_the_scale_it_has_reached() {
        let at = |bytes| {
            RecordingStatus {
                path: PathBuf::new(),
                sentences: 0,
                bytes,
                dropped: 0,
                failed: false,
            }
            .size_label()
        };

        assert_eq!(at(670), "670 B");
        assert_eq!(at(2048), "2.0 kB");
        assert_eq!(at(5 * 1024 * 1024), "5.0 MB");
    }

    /// The whole point: what is captured has to come back through the parser
    /// the same way it went in, damaged sentences included. If a recording
    /// parsed differently from the session it came from, it could not be used
    /// to investigate that session.
    #[tokio::test]
    async fn a_capture_replays_to_the_same_result() {
        let path = TempPath::new("roundtrip");
        let sentences = [
            "$GNGGA,000000.00,5128.67400,N,00000.25200,E,1,16,0.6,46.00,M,45.30,M,,*4D",
            "$GNGSA,A,3,03,04,07,08,09,16,26,,,,,,1.3,0.6,1.0,1*36",
            // Deliberately corrupt: must survive the round trip as a failure.
            "$GNGGA,000001.00,5128.67400,N,00000.25200,E,1,16,0.6,46.00,M,45.30,M,,*00",
        ];

        let recorder = Recorder::start(&path.0).await.unwrap();
        let mut live = crate::state::GnssState::new("live".into());
        let mut live_parser = crate::parser::Aggregator::default();
        for line in sentences {
            recorder.write(line);
            live_parser.ingest_line(&mut live, line);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        drop(recorder);
        tokio::time::sleep(Duration::from_millis(150)).await;

        let mut replayed = crate::state::GnssState::new("replay".into());
        let mut replay_parser = crate::parser::Aggregator::default();
        for line in std::fs::read_to_string(&path.0).unwrap().lines() {
            replay_parser.ingest_line(&mut replayed, line);
        }

        assert_eq!(replayed.counters.total, live.counters.total);
        assert_eq!(replayed.counters.decoded, live.counters.decoded);
        assert_eq!(
            replayed.counters.checksum_failed, live.counters.checksum_failed,
            "a damaged sentence must survive the capture"
        );
        assert_eq!(replayed.counters.checksum_failed, 1);
        assert_eq!(replayed.position(), live.position());
    }

    #[test]
    fn default_names_sort_chronologically() {
        let at =
            |h, m, s| default_filename(chrono::Utc.with_ymd_and_hms(2026, 8, 15, h, m, s).unwrap());
        assert_eq!(at(0, 36, 27), "waypoint-20260815-003627.nmea");
        assert!(at(0, 36, 27) < at(9, 1, 2), "names must sort by time");
    }
}

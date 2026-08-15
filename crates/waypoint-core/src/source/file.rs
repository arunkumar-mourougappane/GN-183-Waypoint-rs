//! File source: load a recorded log at once, or replay it in original time.

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use super::{Cancel, FileMode, SourceEvent};
use crate::state::ConnectionStatus;

/// Clamp on replay sleeps. A log with a large gap (receiver powered off, say)
/// must not stall the UI for real minutes, and a run of identical timestamps
/// must not spin.
const MAX_REPLAY_SLEEP: Duration = Duration::from_secs(2);

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

        if let FileMode::Replay { speed } = mode
            && let Some(epoch_secs) = sentence_time_secs(&line)
        {
            if let Some(previous) = previous_epoch_secs {
                // Guard against a log that wraps past midnight going negative.
                let delta = epoch_secs - previous;
                if delta > 0.0 {
                    let scaled = delta / speed.max(0.01) as f64;
                    let sleep = Duration::from_secs_f64(scaled).min(MAX_REPLAY_SLEEP);
                    tokio::time::sleep(sleep).await;
                }
            }
            previous_epoch_secs = Some(epoch_secs);
        }

        if tx.send(SourceEvent::Line(line)).await.is_err() {
            return;
        }
    }

    let _ = tx
        .send(SourceEvent::Status(ConnectionStatus::Disconnected))
        .await;
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

//! Serial source.
//!
//! `serialport` is blocking, so the read loop lives on a blocking task with a
//! short read timeout — the timeout is what makes cancellation observable
//! without needing to interrupt a blocked read.

use std::io::{ErrorKind, Read};
use std::time::Duration;

use tokio::sync::mpsc;

use super::{Cancel, SourceEvent};
use crate::state::ConnectionStatus;

const READ_TIMEOUT: Duration = Duration::from_millis(100);
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// A line this long means we're not looking at NMEA — almost always a baud rate
/// mismatch producing framing garbage with no line breaks.
const MAX_LINE_BYTES: usize = 4096;

pub async fn run(path: String, baud: u32, tx: mpsc::Sender<SourceEvent>, cancel: Cancel) {
    let handle = tokio::task::spawn_blocking(move || read_loop(path, baud, tx, cancel));
    if let Err(err) = handle.await {
        tracing::error!(%err, "serial reader panicked");
    }
}

fn read_loop(path: String, baud: u32, tx: mpsc::Sender<SourceEvent>, cancel: Cancel) {
    let mut backoff = INITIAL_BACKOFF;
    let mut attempt = 0u32;

    while !cancel.is_cancelled() {
        let status = if attempt == 0 {
            ConnectionStatus::Connecting
        } else {
            ConnectionStatus::Reconnecting { attempt }
        };
        if tx.blocking_send(SourceEvent::Status(status)).is_err() {
            return;
        }

        match serialport::new(&path, baud).timeout(READ_TIMEOUT).open() {
            Ok(port) => {
                attempt = 0;
                backoff = INITIAL_BACKOFF;
                if tx
                    .blocking_send(SourceEvent::Status(ConnectionStatus::Connected))
                    .is_err()
                {
                    return;
                }
                if pump(port, &tx, &cancel).is_err() {
                    return;
                }
            }
            Err(err) => {
                if tx
                    .blocking_send(SourceEvent::Warning(format!("open {path}: {err}")))
                    .is_err()
                {
                    return;
                }
            }
        }

        if cancel.is_cancelled() {
            break;
        }

        attempt += 1;
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }

    let _ = tx.blocking_send(SourceEvent::Status(ConnectionStatus::Disconnected));
}

/// Read until the port errors or cancellation is requested.
/// Returns `Err(())` only when the downstream receiver is gone.
fn pump(
    mut port: Box<dyn serialport::SerialPort>,
    tx: &mpsc::Sender<SourceEvent>,
    cancel: &Cancel,
) -> Result<(), ()> {
    let mut buf = [0u8; 1024];
    let mut pending = String::new();
    // Opening mid-stream usually lands us partway through a sentence; drop bytes
    // until the first line break so the first emitted line is whole.
    let mut synced = false;

    while !cancel.is_cancelled() {
        let read = match port.read(&mut buf) {
            Ok(0) => continue,
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::TimedOut => continue,
            Err(err) => {
                tx.blocking_send(SourceEvent::Warning(format!("serial read: {err}")))
                    .map_err(|_| ())?;
                return Ok(());
            }
        };

        pending.push_str(&String::from_utf8_lossy(&buf[..read]));

        while let Some(idx) = pending.find('\n') {
            let line: String = pending.drain(..=idx).collect();
            if !synced {
                synced = true;
                continue;
            }
            tx.blocking_send(SourceEvent::Line(line.trim().to_string()))
                .map_err(|_| ())?;
        }

        if pending.len() > MAX_LINE_BYTES {
            pending.clear();
            synced = false;
            tx.blocking_send(SourceEvent::Warning(
                "no line breaks in stream — check the baud rate".into(),
            ))
            .map_err(|_| ())?;
        }
    }

    Ok(())
}

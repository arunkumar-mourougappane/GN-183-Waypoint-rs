//! TCP source: network-attached receivers, and serial-over-TCP bridges such as
//! `ser2net` or `socat`.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::{Cancel, SourceEvent};
use crate::state::ConnectionStatus;

const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(15);

pub async fn run(addr: String, tx: mpsc::Sender<SourceEvent>, cancel: Cancel) {
    let mut backoff = INITIAL_BACKOFF;
    let mut attempt = 0u32;

    // Network blips are usually transient, so reconnect indefinitely rather than
    // ending the session and losing the accumulated track.
    while !cancel.is_cancelled() {
        let status = if attempt == 0 {
            ConnectionStatus::Connecting
        } else {
            ConnectionStatus::Reconnecting { attempt }
        };
        let _ = tx.send(SourceEvent::Status(status)).await;

        match TcpStream::connect(&addr).await {
            Ok(stream) => {
                attempt = 0;
                backoff = INITIAL_BACKOFF;
                let _ = tx
                    .send(SourceEvent::Status(ConnectionStatus::Connected))
                    .await;

                if read_stream(stream, &tx, &cancel).await.is_err() {
                    return; // receiver dropped: the engine is shutting down
                }

                if cancel.is_cancelled() {
                    break;
                }
                let _ = tx
                    .send(SourceEvent::Warning("connection closed by peer".into()))
                    .await;
            }
            Err(err) => {
                let _ = tx
                    .send(SourceEvent::Warning(format!("connect {addr}: {err}")))
                    .await;
            }
        }

        attempt += 1;
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }

    let _ = tx
        .send(SourceEvent::Status(ConnectionStatus::Disconnected))
        .await;
}

/// Returns `Err(())` only when the downstream receiver is gone.
async fn read_stream(
    stream: TcpStream,
    tx: &mpsc::Sender<SourceEvent>,
    cancel: &Cancel,
) -> Result<(), ()> {
    let mut lines = BufReader::new(stream).lines();

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        // Wake periodically even on a silent link so cancellation is observed.
        let next = tokio::time::timeout(Duration::from_millis(250), lines.next_line()).await;

        match next {
            Err(_elapsed) => continue,
            Ok(Ok(Some(line))) => {
                tx.send(SourceEvent::Line(line)).await.map_err(|_| ())?;
            }
            Ok(Ok(None)) => return Ok(()),
            Ok(Err(err)) => {
                tx.send(SourceEvent::Warning(format!("read error: {err}")))
                    .await
                    .map_err(|_| ())?;
                return Ok(());
            }
        }
    }
}

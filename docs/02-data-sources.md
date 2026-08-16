# Data Sources

Waypoint needs to treat serial, TCP, and file input uniformly: each is "a stream of
NMEA lines" from the parser's point of view, differing only in how bytes arrive and
what "end of stream" / "error" mean.

## Common abstraction

```rust
pub enum SourceEvent {
    Line(String),           // one raw NMEA sentence, checksum not yet validated
    Error(SourceError),     // recoverable (e.g. serial hiccup) — surfaced, not fatal
    Disconnected,           // source ended (EOF, TCP closed, device unplugged)
}

pub trait NmeaSource {
    // Runs until disconnected/cancelled, pushing SourceEvents to `tx`.
    async fn run(self, tx: mpsc::Sender<SourceEvent>) -> ();
}
```

Each concrete source (serial/TCP/file) implements this and is otherwise invisible
to the rest of the app — the core parser/state layer only ever consumes
`SourceEvent`s. This is the seam that keeps `waypoint-core` transport-agnostic (see
[`03-architecture.md`](03-architecture.md)).

## Live versus recorded

Uniform *ingestion* does not mean a uniform UI. The meaningful split is not
serial-vs-TCP but **live-vs-recorded**, and the two afford opposite things:

| | Live (serial, TCP) | Recorded (file) |
|---|---|---|
| Time base | Real, unrepeatable | Known in advance, replayable |
| Can be paused / retimed / restarted | No | Yes |
| Has a position and a length | No | Yes |
| Can go stale or drop out | Yes | No — it ends, which is not a fault |
| Acquisition metrics (time to first fix) | Meaningful | Meaningless: the log already contains a fix |

`SourceConfig::controls()` returns a `SourceControls` (`Live`, `Replay`, or
`Instant`) and the frontends branch on that, so neither offers a control the
source could not honour: no pause button on a receiver, no staleness warning on
a finished log.

The vocabulary follows the same split. A file is *opened* and reaches an *end of
log*; only a receiver *connects* and *disconnects*, and only its dropping out is
an error worth colouring red. `ConnectionStatus::label_for` and
`ConnectionStatus::tone` carry that wording and colouring so both frontends
phrase it identically, while the plain `label()` stays technical for logs. See [`05-ui-gui.md`](05-ui-gui.md) and
[`06-ui-tui.md`](06-ui-tui.md) for what each surfaces.

## Serial

- Crate: [`serialport`](https://crates.io/crates/serialport) (v4.9.0) for a
  blocking-read-in-a-dedicated-thread approach, or
  [`tokio-serial`](https://crates.io/crates/tokio-serial) (v5.5.0, wraps
  `mio-serial`) if the app is async end-to-end via `tokio`.
- Config needed: port path, baud rate (commonly 4800 for older/basic GPS, 9600 or
  38400/115200 for many modern modules — must be user-specified, not guessed),
  8N1 framing (near-universal default).
- Read loop: wrap the port in a `BufReader`, split on `\n`. NMEA lines can be
  interleaved with noise on first connect (partial sentence from before the port
  opened) — discard bytes until the first `$` is seen.
- Error handling: a serial device unplugged mid-session should surface as
  `SourceEvent::Disconnected`, not panic; the UI should show "disconnected,
  waiting" and allow reconnect/re-select rather than exiting.

## TCP

- Crate: `tokio::net::TcpStream` (async) or `std::net::TcpStream` + a reader thread
  if the app stays sync. Given the GUI/TUI split already implies an async core (to
  avoid blocking either frontend's event loop), prefer `tokio::net::TcpStream`.
- Use case: NTRIP-adjacent setups, network-attached GNSS receivers, or `socat`/`ser2net`
  bridges exposing a serial GPS over a TCP port — common for bench debugging.
- Same line-splitting approach as serial, via `tokio::io::BufReader` +
  `AsyncBufReadExt::lines()`.
- Reconnect policy: TCP source should support a bounded retry-with-backoff on
  connection drop, since network blips are more common than serial disconnects and
  usually transient.

## File (static log or replay)

- Two modes worth distinguishing in the UI. Replay is the default: a log opened
  without a mode arrives with a working transport, where an instant load would
  present a frontend with nothing to pause, retime or scrub and look broken.
  - **Instant load** (`--instant`): parse the whole file, compute trip stats,
    show the full track immediately — useful for "what happened on this log".
  - **Replay**: pace sentence emission using the timestamps embedded in
    RMC/GGA/ZDA, so the live map/dashboard animate the same way they would from
    a real device. This is what makes the file source useful for testing the
    GUI/TUI without hardware.

    Transport state lives in a `ReplayControl` handle shared with the frontend:
    rate, pause, and position. Restarting the source to change any of them would
    discard the accumulated track and trip statistics, which is exactly what
    someone studying a log does not want. Waits are taken in short chunks so a
    change lands within a tick instead of after the current inter-sentence gap
    finishes, and pausing holds without consuming log time so resuming picks the
    pacing up where it left off.

    Position is byte-based, read through a seekable `BufReader` with `read_line`
    so the byte count is measured rather than estimated. That also makes the
    recording random-access: `ReplayControl::seek_to` asks the source to jump to
    a fraction of the log, which is what backs the scrub bar in both frontends.

    Seeking by byte lands mid-sentence, so the remainder of that line is
    discarded — emitting half a sentence would be counted as a checksum failure
    and read as corruption that is not there. After a jump the source runs on
    unpaced until a GGA closes an epoch, so scrubbing while paused leaves the
    dashboard on a complete fix rather than a fragment. Everything derived from
    the old position — track, trip, satellites — is cleared, because it
    describes a stretch of log that is no longer where we are.
- Implementation: buffered file read, line-by-line; for replay, sleep between
  lines based on the delta between consecutive fix timestamps, capped so a log
  with a large gap (receiver switched off) doesn't stall the UI for real
  minutes.
- Malformed lines (partial writes, non-NMEA log noise) should be skipped with a
  logged warning, not treated as fatal — logs from real hardware are rarely
  perfectly clean.

## Cross-cutting concerns

- **Checksum validation** happens once, centrally, right after `SourceEvent::Line`
  is received — regardless of which source produced it — so serial/TCP/file share
  one validation path and one "sentences seen / sentences rejected" counter for
  the debugger's own health display.
- **Source switching at runtime**: the UI should let the user change source
  (serial port, TCP host:port, or file path) without restarting the app — the
  `NmeaSource` abstraction makes this a matter of tearing down one task and
  spawning another against the same downstream channel.

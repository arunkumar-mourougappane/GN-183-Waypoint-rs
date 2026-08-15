# Architecture

## Workspace layout

A Cargo workspace splitting shared logic from each frontend, so the TUI build
doesn't pull in `egui`/`walkers` and vice versa:

```
GN-183-Waypoint-rs/
├── Cargo.toml              # [workspace]
├── crates/
│   ├── waypoint-core/       # sources, parsing, state, trip stats — no UI deps
│   │   └── src/
│   │       ├── source/      # mod.rs (NmeaSource, Cancel), serial.rs, tcp.rs, file.rs
│   │       ├── parser.rs    # checksum + Aggregator (GSV reassembly, GSA cycles)
│   │       ├── state.rs     # GnssState, TrackPoint, SatelliteInfo, FixMode/FixQuality
│   │       ├── trip.rs      # TripStats, TripConfig, haversine
│   │       └── engine.rs    # Engine: source -> parser -> state -> frontends
│   ├── waypoint-gui/        # eframe/egui + walkers + egui_plot binary
│   └── waypoint-tui/        # ratatui binary
├── assets/sample.nmea       # synthetic drive log for testing without hardware
└── docs/
```

`waypoint-core` has zero knowledge of either frontend. Both binaries depend on it
and on nothing GUI/TUI-specific from each other.

## `waypoint-core` responsibilities

1. **Sources** — `NmeaSource` implementations for serial/TCP/file (see
   [`02-data-sources.md`](02-data-sources.md)), each pushing raw lines into an
   `mpsc` channel.
2. **Parsing** — wraps the `nmea` crate's stateful parser; validates checksums;
   converts talker ID + sentence into typed events.
3. **State aggregation** — a `GnssState` struct holding the latest fix
   (position, altitude, speed, course, fix type/quality, HDOP), the satellite
   table (per constellation, from reassembled GSV groups), and the accumulated
   track (`Vec<TrackPoint>` with timestamp).
4. **Trip stats engine** — incrementally updates distance/time/speed stats as new
   fixes arrive (see [`07-trip-statistics.md`](07-trip-statistics.md)), rather
   than recomputing from scratch every update.
5. **Publishing to frontends** — `Engine` holds the state in an
   `Arc<RwLock<GnssState>>` and publishes a monotonic version counter over a
   `tokio::sync::watch`. Frontends call `Engine::state()` for a read guard to
   render from, and wait on `Engine::updates()` to know when to redraw.

   The state is *not* cloned into the channel: the track holds up to 200,000
   points, and copying that once per fix would cost far more than the parsing.
   The watch carries only the version number, so the notification stays cheap
   and a slow frontend coalesces a burst into one redraw.

## Concurrency model

- `waypoint-core` runs on a `tokio` runtime: one task per active source, and one
  ingest task applying parsed events to the shared `GnssState`. The ingest task
  never holds the write lock across an `.await`, so a frame's read guard can
  never be blocked for long.
- **GUI frontend** (`eframe`): eframe owns its own event loop on the main thread;
  the app owns a `tokio::runtime::Runtime` and spawns through its `Handle`. A
  small task watches `Engine::updates()` and calls `ctx.request_repaint()`, so
  the GUI redraws when data arrives rather than spinning at 60 fps for a source
  that emits about once a second.
- **TUI frontend** (`ratatui`): similar shape — an async event loop (`tokio`
  main) selecting between terminal input events (crossterm) and state-change
  notifications, redrawing on either.
- Neither frontend blocks on I/O directly; all source I/O lives in
  `waypoint-core` tasks.

## Why not a single always-on binary with both UIs live

Running GUI and TUI simultaneously against the same core is architecturally free
(both are just subscribers to the same broadcast channel) but is explicitly out of
scope for the initial build — pick a frontend via CLI subcommand/flag
(`waypoint gui`, `waypoint tui`) at launch. Revisit if there's a real use case
(e.g. TUI on a field laptop's SSH session while GUI runs locally against the same
log file) later.

## Error/reconnect handling

Source errors (`SourceEvent::Error`/`Disconnected`, see
[`02-data-sources.md`](02-data-sources.md)) propagate up through `GnssState` as a
connection-status field, not as a crash — both frontends render a
"disconnected/reconnecting" indicator rather than losing the accumulated track and
trip stats when a serial cable is bumped or a TCP link blips.

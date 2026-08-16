# gn183-waypoint-core

Transport-agnostic NMEA 0183 ingestion, parsing and GNSS state aggregation — the
library behind [Waypoint](https://github.com/arunkumar-mourougappane/GN-183-Waypoint-rs),
an NMEA debugger with a map dashboard and a terminal dashboard.

It knows nothing about how the data is displayed. Give it a source, read the
state, render it however you like.

```rust
use waypoint_core::{Engine, SourceConfig, FileMode, ReplayControl, TripConfig};

// Inside a Tokio runtime: the engine spawns its source and ingest tasks.
async fn watch() {
    let engine = Engine::new(TripConfig::default());
    engine.set_source(SourceConfig::File {
        path: "drive.nmea".into(),
        mode: FileMode::Replay { control: ReplayControl::new(4.0) },
    });

    let mut updates = engine.updates();
    while updates.changed().await.is_ok() {
        let state = engine.state();
        println!("{} sats, fix {}", state.sats_in_view(), state.fix_mode.label());
    }
}
```

Note the import: the package is `gn183-waypoint-core` on crates.io — plain
`waypoint-core` was already taken — but the library it exposes is `waypoint_core`,
so that is what `use` refers to.

## What it gives you

- **Sources** — serial, TCP and log files behind one interface, with reconnect
  and backoff on the live ones.
- **Parsing** — GGA, RMC, GSA, GSV, VTG, GLL, GST and ZDA, with checksums
  validated centrally and stream-health counters kept for every rejection.
- **State** — the current fix, a satellite table reassembled per constellation,
  a bounded track and metric history, and a ring of raw sentences.
- **Trip statistics** — distance, moving versus stopped time, speed, elevation
  gain and loss, all accumulated in O(1) per fix and guarded against fixes that
  imply impossible speeds.
- **Replay control** — pause, retime and seek a recording while it plays.
- **Recording** — capture a live stream to a log that replays, verbatim,
  including sentences that failed their checksum.

## Two design notes

**A live receiver and a recorded log are not the same thing.**
`SourceConfig::controls()` reports `Live`, `Replay` or `Instant`, so a frontend
can offer only what the source can honour: a recording can be paused and
scrubbed, a receiver can fall silent and has to acquire a fix.

**The `nmea` crate's stateless API is used, not its stateful parser.** The
stateful one merges sentences for you but drops GSA's 2D/3D fix mode and does
not group GSV by talker — both of which a dashboard needs. `Aggregator` does
that bookkeeping instead, including the cycle handling that multi-constellation
receivers require.

## The frontends

- [`gn183-waypoint-tui`](https://crates.io/crates/gn183-waypoint-tui) — terminal
  dashboard, with the track and a road basemap drawn in braille.
- [`gn183-waypoint-gui`](https://crates.io/crates/gn183-waypoint-gui) — map
  dashboard on OpenStreetMap tiles.

## License

MIT.

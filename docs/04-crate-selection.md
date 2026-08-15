# Crate Selection

Versions below are what the workspace actually builds against (see `Cargo.lock`),
verified on 2026-08-15. The `egui` ecosystem in particular ships frequently — check
before bumping.

| Crate | Version | Used in | Purpose / rationale |
|---|---|---|---|
| [`nmea`](https://crates.io/crates/nmea) | 0.8.0 | core | NMEA 0183 parsing — GGA/RMC/GSA/GSV/VTG/GLL/GST/ZDA coverage plus a `Satellite` type for GSV data. Avoids hand-rolling field parsing. Used via the *stateless* `parse_str` API, not its stateful `Nmea` aggregator — see below. |
| [`tokio`](https://crates.io/crates/tokio) | 1.53.1 | core, both frontends | Async runtime for source I/O, channels (`mpsc`/`watch`), and timers (replay pacing, reconnect backoff). |
| [`serialport`](https://crates.io/crates/serialport) | 4.9.0 | core | Cross-platform serial I/O. Blocking, so the read loop runs on `spawn_blocking` with a 100 ms read timeout — the timeout is what makes cancellation observable without interrupting a blocked read. |
| [`eframe`](https://crates.io/crates/eframe) / [`egui`](https://crates.io/crates/egui) | 0.36.1 | waypoint-gui | Native app shell and immediate-mode UI. Note 0.36 replaced `App::update(ctx, frame)` with `App::ui(ui, frame)` and unified `SidePanel`/`TopBottomPanel` into `egui::Panel`. |
| [`walkers`](https://crates.io/crates/walkers) | 0.58.0 | waypoint-gui | Slippy-map widget for `egui`: OSM/XYZ tile servers, offline `.pmtiles`, vector (MVT) tiles. The track is drawn over it through the `Plugin` trait. |
| [`egui_plot`](https://crates.io/crates/egui_plot) | 0.37.0 | waypoint-gui | Time-series plots for speed/altitude/HDOP. |
| [`ratatui`](https://crates.io/crates/ratatui) | 0.30.2 | waypoint-tui | Terminal UI: `Canvas` (braille track plot), `Sparkline`, `Table`. Re-exports `crossterm` for input, and `TestBackend` makes the dashboard renderable in a unit test with no TTY. |
| [`chrono`](https://crates.io/crates/chrono) | 0.4.45 | core, frontends | Timestamps. Chosen over `time` because the `nmea` crate already returns `NaiveTime`/`NaiveDate`, so there is no conversion layer. |
| [`clap`](https://crates.io/crates/clap) | 4.6.6 | both frontends | Source selection flags (`--serial`/`--baud`, `--tcp`, `--file`/`--replay`/`--speed`, `--list-ports`). |
| `tracing` + `tracing-subscriber` | 0.1 / 0.3 | all | Structured logging. The TUI only enables it when `WAYPOINT_LOG` names a file, since log output would corrupt the alternate screen. |
| `thiserror`, `anyhow` | 2 / 1 | core / binaries | Error types in the library, error reporting in the binaries. |

## Deliberately not chosen

- **`geo`** — the original plan called for it, but the only thing needed is a
  haversine distance. Pulling in `geo` brings `spade`, `rstar`, `i_overlay`,
  `earcut` and `geographiclib-rs` for one function; `haversine_distance_m` in
  `waypoint-core/src/trip.rs` is ten lines and unit-tested against a known
  city-pair distance. Revisit if real geospatial operations (bounding-box
  queries, simplification, geodesic accuracy) become necessary.
- **`nmea`'s stateful `Nmea` parser** — convenient, but it does not retain GSA's
  2D/3D fix mode (`GsaMode2`) and does not expose per-talker GSV grouping, both of
  which the dashboard needs. `waypoint-core`'s `Aggregator` does that
  bookkeeping over the stateless `parse_str` API instead.
- **`tokio-serial`** — would avoid the blocking bridge thread, but couples the
  serial layer to a second serialport-derived dependency tree for a stream that
  emits a handful of lines per second. Not worth it.
- **Web dashboard (Leaflet.js + WebSocket server)** — ruled out per the explicit
  requirement of a native GUI plus a TUI, no browser dependency.
- **AIS support** — `nmea-parser` and `nmea-kit` handle AIS as well as GNSS; out
  of scope per [`00-overview.md`](00-overview.md), but worth knowing they exist.

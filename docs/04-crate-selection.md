# Crate Selection

Versions are what the workspace resolves to in `Cargo.lock`, verified 2026-08-15.
The `egui` ecosystem ships frequently — re-check before bumping.

## Shared

| Crate | Version | Purpose / rationale |
|---|---|---|
| [`nmea`](https://crates.io/crates/nmea) | 0.8.0 | NMEA 0183 parsing: GGA/RMC/GSA/GSV/VTG/GLL/GST/ZDA plus a `Satellite` type for GSV. Used through the *stateless* `parse_str`, not its stateful aggregator — see below. |
| [`tokio`](https://crates.io/crates/tokio) | 1.53.1 | Async runtime for source I/O, channels (`mpsc`/`watch`) and timers (replay pacing, reconnect backoff). |
| [`chrono`](https://crates.io/crates/chrono) | 0.4.45 | Timestamps. Chosen over `time` because `nmea` already returns `NaiveTime`/`NaiveDate`, so there is no conversion layer. **Note:** `%.2f` is not a valid specifier; its `Display` returns `Err` and `to_string` panics. That has bitten this codebase twice — use `%.3f`. |
| [`clap`](https://crates.io/crates/clap) | 4.6.6 | Source-selection flags, identical in shape across both frontends. |
| `tracing` + `tracing-subscriber` | 0.1 / 0.3 | Structured logging. The TUI only enables it when `WAYPOINT_LOG` names a file, since log output would corrupt the alternate screen. |
| `thiserror`, `anyhow` | 2 / 1 | Error types in the library, error reporting in the binaries. |

## `waypoint-core`

| Crate | Version | Purpose |
|---|---|---|
| [`serialport`](https://crates.io/crates/serialport) | 4.9.0 | Cross-platform serial I/O. Blocking, so the read loop runs on `spawn_blocking` with a 100 ms read timeout — the timeout is what makes cancellation observable without interrupting a blocked read. |

## `waypoint-gui`

| Crate | Version | Purpose |
|---|---|---|
| [`eframe`](https://crates.io/crates/eframe) / [`egui`](https://crates.io/crates/egui) | 0.36.1 | App shell and immediate-mode UI. 0.36 replaced `App::update(ctx, frame)` with `App::ui(ui, frame)` and unified `SidePanel`/`TopBottomPanel` into `egui::Panel`. |
| [`walkers`](https://crates.io/crates/walkers) | 0.58.0 | Slippy-map widget: OSM/XYZ tiles, offline `.pmtiles`, vector tiles. The track is drawn over it through the `Plugin` trait. |
| [`egui_plot`](https://crates.io/crates/egui_plot) | 0.37.0 | Time-series plots for speed, altitude and HDOP. |
| [`rfd`](https://crates.io/crates/rfd) | 0.17.2 | Native file dialog for choosing a log, instead of a typed path. |

## `waypoint-tui`

| Crate | Version | Purpose |
|---|---|---|
| [`ratatui`](https://crates.io/crates/ratatui) | 0.30.2 | Terminal UI: `Canvas` (braille track and basemap), `Sparkline`, `Table`. Re-exports `crossterm` for input, and `TestBackend` makes the whole dashboard renderable in a unit test with no TTY. |
| [`mvt-reader`](https://crates.io/crates/mvt-reader) | 2.4.0 | Decodes Mapbox Vector Tiles for the basemap. **Note:** geometry comes back in tile-local units spanning the layer's `extent` (4096 by convention), *not* normalised — assuming otherwise projects every road off the map. |
| [`reqwest`](https://crates.io/crates/reqwest) | 0.12.28 | Tile fetching, `rustls` only — no OpenSSL anywhere in the tree. |
| [`directories`](https://crates.io/crates/directories) | 6.0.0 | Platform cache location for tiles: `~/Library/Caches`, `%LOCALAPPDATA%`, `$XDG_CACHE_HOME`. |
| [`geo-types`](https://crates.io/crates/geo-types) | 0.7.20 | The geometry types `mvt-reader` returns. Types only — no algorithms, no heavy tree. |

## Deliberately not chosen

- **`geo`** — the plan called for it, but the only thing needed was a haversine
  distance, and it pulls in `spade`, `rstar`, `i_overlay`, `earcut` and
  `geographiclib-rs` to provide it. `haversine_distance_m` in
  `waypoint-core/src/trip.rs` is ten lines, unit-tested against a known
  city-pair distance. It was left declared-but-unused for a while, quietly
  dragging all five in; removing the declaration dropped every one of them from
  the lock file. Revisit only if real geospatial operations arrive.
- **`nmea`'s stateful `Nmea` parser** — convenient, but it does not retain GSA's
  2D/3D fix mode (`GsaMode2`) and does not expose per-talker GSV grouping, both
  of which the dashboard needs. `waypoint-core`'s `Aggregator` does that
  bookkeeping over the stateless API instead.
- **`tokio-serial`** — would avoid the blocking bridge thread, but couples the
  serial layer to a second serialport-derived dependency tree for a stream that
  emits a handful of lines per second.
- **`ratatui-image`** (sixel/kitty graphics) — would show real map *imagery* in
  capable terminals, but degrades to unusable halfblocks elsewhere and cannot
  compose with a braille track overlay. Vector geometry draws on any terminal;
  see [`06-ui-tui.md`](06-ui-tui.md).
- **A JSON crate for TileJSON** — one field is wanted from a fixed-shape
  document from a known server; a twenty-line scan beats a dependency.
- **Web dashboard (Leaflet + WebSocket)** — ruled out by the requirement for a
  native GUI plus a TUI, no browser.
- **AIS support** — `nmea-parser` and `nmea-kit` handle it; out of scope per
  [`00-overview.md`](00-overview.md).

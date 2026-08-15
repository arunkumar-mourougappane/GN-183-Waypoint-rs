# GN-183 Waypoint

[![CI](https://github.com/arunkumar-mourougappane/GN-183-Waypoint-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/arunkumar-mourougappane/GN-183-Waypoint-rs/actions/workflows/ci.yml)

GN-183 Waypoint is a high-precision telemetry and NMEA 0183 debugging suite built in Rust. Designed to ingest raw GPS data streams via Serial COM, TCP networks, or static log files, Waypoint translates continuous comma-separated payloads into a real-time navigational dashboard.

Engineered for speed and memory safety, it allows developers to visually plot geodetic coordinates on a live tracking map while simultaneously monitoring critical array parameters like HDOP, satellite lock geometry, speed, altitude, and 2D/3D fix states.

## Layout

```
crates/
├── waypoint-core/   sources, parsing, GNSS state, trip statistics — no UI deps
├── waypoint-gui/    eframe/egui dashboard with a walkers slippy map
└── waypoint-tui/    ratatui dashboard for headless/SSH use
```

Both frontends render the same `GnssState` from `waypoint-core`; neither depends on
the other's dependencies. See [`docs/`](docs/) for the protocol reference and design
notes.

## Running

Replay the bundled sample log at 10× speed — no hardware needed:

```sh
cargo run -p waypoint-gui -- --file assets/sample.nmea --replay --speed 10
cargo run -p waypoint-tui -- --file assets/sample.nmea --replay --speed 10
```

Live serial, TCP, or a log loaded all at once:

```sh
cargo run -p waypoint-gui -- --list-ports
cargo run -p waypoint-gui -- --serial /dev/tty.usbserial-0001 --baud 9600
cargo run -p waypoint-tui -- --tcp 192.168.1.50:10110
cargo run -p waypoint-tui -- --file drive.nmea          # no --replay: parse immediately
```

The GUI can also be started with no source and configured from its **Source…** panel
(serial port picker, TCP address, or a native file chooser).

| Flag | Meaning |
|---|---|
| `--serial <PORT> [--baud N]` | Read from a serial port (default 9600 baud) |
| `--tcp <HOST:PORT>` | Read from a TCP endpoint, with reconnect backoff |
| `--file <PATH>` | Read a recorded log |
| `--replay [--speed X]` | Pace a log by its own timestamps instead of loading it at once |
| | Without `--replay` a log is parsed as fast as it can be read |
| `--list-ports` | Print available serial ports and exit |

### Live sources versus recordings

The two are not interchangeable, and the UI does not pretend they are. A
recording can be paused, retimed and restarted, and has a position within a
known length. A receiver has none of those, but it can fall silent, and how long
it took to get a fix matters. Each frontend offers only what the running source
can honour:

| | Serial / TCP | File replay | File, instant |
|---|---|---|---|
| Pause, restart, rate | — | ✓ | — |
| Position in log | — | ✓ | — |
| Data rate, stale warning | ✓ | — | — |
| Time to first fix | ✓ | — | — |

Rate and pause apply to a replay already in progress, without restarting it and
losing the track accumulated so far.

### TUI keys

`q` quit · `r` reset trip · `n` toggle the plots/raw-sentence panel.

While replaying: `space` pause/resume · `-`/`+` halve/double the rate ·
`1` back to real time · `R` restart from the beginning.

The TUI writes no logs to the terminal (they would corrupt the display); set
`WAYPOINT_LOG=/path/to/file` plus `RUST_LOG` to capture them.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

`waypoint-tui` renders its whole dashboard through ratatui's `TestBackend`, so the
layout is covered by tests without needing a terminal.

CI runs those same three commands on every push and pull request, with the test
job spanning Linux, macOS and Windows. Building on Linux needs a few system
packages that the other platforms ship with their SDKs — `libudev-dev` for
`serialport`, and the xkb/xcb headers for the windowing stack; see
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) for the exact list.

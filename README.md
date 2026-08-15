# GN-183 Waypoint

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
| `--list-ports` | Print available serial ports and exit |

### TUI keys

`q` quit · `r` reset trip · `n` toggle the plots/raw-sentence panel.

The TUI writes no logs to the terminal (they would corrupt the display); set
`WAYPOINT_LOG=/path/to/file` plus `RUST_LOG` to capture them.

## Development

```sh
cargo test --workspace
```

`waypoint-tui` renders its whole dashboard through ratatui's `TestBackend`, so the
layout is covered by tests without needing a terminal.

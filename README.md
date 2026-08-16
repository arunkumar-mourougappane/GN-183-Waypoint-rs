# GN-183 Waypoint

[![CI](https://github.com/arunkumar-mourougappane/GN-183-Waypoint-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/arunkumar-mourougappane/GN-183-Waypoint-rs/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/arunkumar-mourougappane/GN-183-Waypoint-rs?sort=semver)](https://github.com/arunkumar-mourougappane/GN-183-Waypoint-rs/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platforms-linux%20%7C%20macos%20%7C%20windows-lightgrey.svg)](#running)

GN-183 Waypoint is a high-precision telemetry and NMEA 0183 debugging suite built in Rust. Designed to ingest raw GPS data streams via Serial COM, TCP networks, or static log files, Waypoint translates continuous comma-separated payloads into a real-time navigational dashboard.

Engineered for speed and memory safety, it allows developers to visually plot geodetic coordinates on a live tracking map while simultaneously monitoring critical array parameters like HDOP, satellite lock geometry, speed, altitude, and 2D/3D fix states.

It was built to work with [tab5-gps-monitor](https://github.com/arunkumar-mourougappane/tab5-gps-monitor) — an M5Stack Tab5 (ESP32-P4) driving an AT6668 GPS/BDS unit — which is why the three source types are the three ways that device emits NMEA. See [Companion hardware](#companion-hardware).

## Layout

```
crates/
├── waypoint-core/   sources, parsing, GNSS state, trip statistics — no UI deps
├── waypoint-gui/    eframe/egui dashboard with a walkers slippy map
└── waypoint-tui/    ratatui dashboard for headless/SSH use, with a braille basemap
```

Both frontends render the same `GnssState` from `waypoint-core`; neither depends
on the other's dependencies, which is what keeps the terminal build usable on a
machine with no windowing system.

On crates.io the packages are `gn183-waypoint-core`, `gn183-waypoint-gui` and
`gn183-waypoint-tui` — named after the repository, because plain `waypoint-core`
belongs to an unrelated crate. The binaries you type stay `waypoint-gui` and
`waypoint-tui`, and the library is still `waypoint_core` in code.

```sh
cargo install gn183-waypoint-tui
```

[`docs/`](docs/) covers the protocol, the design and the reasoning:
[overview](docs/00-overview.md) ·
[NMEA reference](docs/01-nmea-protocol.md) ·
[data sources](docs/02-data-sources.md) ·
[architecture](docs/03-architecture.md) ·
[dependencies](docs/04-crate-selection.md) ·
[GUI](docs/05-ui-gui.md) · [TUI](docs/06-ui-tui.md) ·
[trip statistics](docs/07-trip-statistics.md) ·
[roadmap](docs/08-roadmap.md) · [testing](docs/09-testing.md)

## Running

Replay the bundled sample log — no hardware needed. A log replays in its original
timing by default, so it arrives with a working transport: pause it, retime it,
scrub through it.

```sh
cargo run -p waypoint-gui -- --file assets/sample.nmea
cargo run -p waypoint-tui -- --file assets/sample.nmea --speed 10
```

Live serial, TCP, or a log loaded all at once:

```sh
cargo run -p waypoint-gui -- --list-ports
cargo run -p waypoint-gui -- --serial /dev/tty.usbserial-0001 --baud 9600
cargo run -p waypoint-tui -- --tcp 192.168.1.50:10110
cargo run -p waypoint-tui -- --file drive.nmea --instant   # skip to the finished track
```

The GUI can also be started with no source and configured from its **Source…** panel
(serial port picker, TCP address, or a native file chooser).

| Flag | Meaning |
|---|---|
| `--serial <PORT> [--baud N]` | Read from a serial port (default 9600 baud) |
| `--tcp <HOST:PORT>` | Read from a TCP endpoint, with reconnect backoff |
| `--file <PATH>` | Read a recorded log |
| `--speed X` | Replay rate; a log replays at real time by default |
| `--instant` | Parse a log as fast as it can be read, skipping to the finished track. No transport, since there is nothing left to control |
| `--record <PATH>` | Capture a live stream to a log you can replay later. Live sources only |
| `--list-ports` | Print available serial ports and exit |
| `--no-map` | TUI only: skip the basemap and make no network requests |

### Companion hardware

[tab5-gps-monitor](https://github.com/arunkumar-mourougappane/tab5-gps-monitor)
runs on an M5Stack Tab5 (ESP32-P4) with an M5Stack GPS/BDS unit (AT6668) on
Port.A. It emits NMEA three ways, and Waypoint reads all three:

| The Tab5 does this | Waypoint reads it with |
|---|---|
| Streams NMEA 0183 over UART at 115200 baud | `--serial <port> --baud 115200` |
| Serves the sentence stream over its `Tab5-GPS` Wi-Fi AP on TCP port 10110 | `--tcp <tab5-address>:10110` |
| Writes raw NMEA and decoded track points to SD | `--file gps_0001.nmea` |

```sh
# Over USB
cargo run -p waypoint-tui -- --serial /dev/tty.usbserial-0001 --baud 115200

# Over its Wi-Fi AP — join `Tab5-GPS` first; the device's address on that
# network is not fixed by this project, so check the Tab5's own display.
cargo run -p waypoint-gui -- --tcp 192.168.4.1:10110

# From a log pulled off the SD card
cargo run -p waypoint-gui -- --file gps_0001.nmea
```

TCP port 10110 is the conventional port for NMEA over IP, which is why it is the
default in the GUI's source picker.

That pairing is also where the parser's harder cases came from: the AT6668
reports GPS, GLONASS, Galileo, BeiDou and QZSS, emits one `GNGSA` per
constellation under a single talker, and timestamps that drift off the whole
second — all of which broke something here before they were handled.

### Live sources versus recordings

The two are not interchangeable, and the UI does not pretend they are. A
recording can be paused, retimed and restarted, and has a position within a
known length. A receiver has none of those, but it can fall silent, and how long
it took to get a fix matters. Each frontend offers only what the running source
can honour:

| | Serial / TCP | File replay | File, instant |
|---|---|---|---|
| Pause, restart, rate | — | ✓ | — |
| Seekable scrub bar | — | ✓ | — |
| Data rate, stale warning | ✓ | — | — |
| Time to first fix | ✓ | — | — |
| Record to a replayable log | ✓ | — | — |

Rate and pause apply to a replay already in progress, without restarting it and
losing the track accumulated so far.

Recording closes the loop between the two: capture a receiver misbehaving in the
field, then pause, rewind and scrub over the failure at a desk. What is written
is the raw stream verbatim, damaged sentences included — a recording that were
tidier than the wire could not reproduce the fault worth recording.

The terminal draws a basemap too — roads, water and green space from vector
tiles, stroked onto the same braille canvas as the track. Tiles are fetched when
the network is reachable and read from a platform cache when it is not, so a
session works at a desk and in a field with no signal; `--no-map` turns it off
and makes no network requests.

Both frontends show the same figures. The polar sky view, map zoom and the
runtime source picker are GUI-only — see
[`docs/06-ui-tui.md`](docs/06-ui-tui.md) for why each is a deliberate omission
rather than a gap.

### TUI keys

`q` quit · `r` reset trip · `n` toggle the plots/raw-sentence panel.

On a live source: `w` starts and stops recording, into a timestamped file in the
working directory.

While replaying: `space` pause/resume · `←`/`→` scrub ±5% · `Home` jump to the
start · `-`/`+` halve/double the rate · `1` back to real time · `R` restart.

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

`assets/sample.nmea` is generated by `assets/generate_sample.py` and is shaped
like real receiver output — drifting sub-second timestamps, five GSA sentences
per epoch, untracked satellites, damaged sentences. Regenerate it with
`python3 assets/generate_sample.py`. Every oddity is deliberate:
[`docs/09-testing.md`](docs/09-testing.md) explains which bug each one exists to
catch.

CI runs those same three commands on every push and pull request, with the test
job spanning Linux, macOS and Windows. Building on Linux needs a few system
packages that the other platforms ship with their SDKs — `libudev-dev` for
`serialport`, and the xkb/xcb headers for the windowing stack; see
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) for the exact list.

Requires **Rust 1.88 or newer** — let-chains are used throughout, and they
stabilised there in the 2024 edition.

# Waypoint v0.1.0

First release. An NMEA 0183 debugger that reads from a serial port, a TCP
endpoint or a recorded log, and shows what the receiver is actually doing — on a
map, and in a terminal.

It is a **debugger**, not a tracker. It keeps what a tracker throws away: the raw
sentences, the checksum failures, the parse errors, the satellites in view but
unused, the dead-reckoned fix that should not count toward distance.

## What's in it

**Two frontends over one core.** `waypoint-gui` draws the track on OpenStreetMap
tiles; `waypoint-tui` draws it in braille over a vector-tile basemap of roads and
water. Both render the same state and report the same figures.

**Sources.** Serial (with reconnect and a baud-rate warning when the stream has
no line breaks), TCP (with backoff), and log files — the three ways
[tab5-gps-monitor](https://github.com/arunkumar-mourougappane/tab5-gps-monitor)
emits NMEA, which is the hardware this was written for.

**Recording closes the loop.** A live stream can be captured to a log the file
source replays, so a receiver misbehaving in the field can be pushed, paused and
scrubbed at a desk. Live sources only, and written verbatim — damaged sentences
included, since those are usually the reason for recording.

**Live and recorded are treated differently**, because they are different. A
recording gets a transport — pause, rate, restart, and a scrub bar that seeks
rather than merely reporting, working while paused. A receiver gets liveness
instead: sentences per second, a warning when the stream goes quiet, and time to
first fix. Neither is offered what it cannot honour, down to the wording: a log
is *opened* and reaches an *end of log*; only a link *connects* and *disconnects*.

**The GNSS detail a debugger needs.** 2D/3D fix mode from GSA and fix quality
from GGA, DOP with quality bands, satellites broken out by constellation with
used-in-fix marked, a polar sky view (GUI) or satellite table (terminal), speed
/ altitude / HDOP plots, and trip statistics guarded against bad fixes.

**Trip statistics that resist bad data.** Distance, moving versus stopped time,
average and maximum speed, elevation gain and loss, average reception quality.
Position jumps implying impossible speeds are excluded from the totals but stay
visible in the track — this is a debugger, so bad fixes are worth seeing.

## Requirements

- **Rust 1.88 or newer** (let-chains, 2024 edition).
- Linux additionally needs `libudev-dev` for serial enumeration and the xkb/xcb
  headers for the windowing stack. macOS and Windows get these from their SDKs.
  The exact list is in `.github/workflows/ci.yml`.

## Getting started

No hardware needed — a sample log ships with it:

```sh
cargo run -p waypoint-gui -- --file assets/sample.nmea
cargo run -p waypoint-tui -- --file assets/sample.nmea --speed 10
```

A log replays in its original timing by default, so it arrives with a working
transport. `--instant` skips to the finished track instead. See the README for
the full flag list and the terminal's keys.

## Known limitations

Worth reading before trusting it with something important.

- **Serial and TCP have not been exercised against real hardware.** The code
  paths compile, are unit-tested, and handle reconnection, but every end-to-end
  run so far has been from a file. This is the most likely place for a surprise,
  and [tab5-gps-monitor](https://github.com/arunkumar-mourougappane/tab5-gps-monitor)
  is the device to try first: UART at 115200, or TCP 10110 over its `Tab5-GPS`
  access point.
- **Correctness is checked against a generated fixture**, which proves the
  parser handles real receiver *shapes* — fractional timestamps, multi-talker
  GSA, untracked satellites, damaged sentences — but not that it produces
  correct *coordinates*, since both sides of that comparison are written here.
  Validating against a recording published with its own decoded track is an open
  item.
- **The terminal basemap needs the network at least once.** Tiles are cached to
  the platform cache directory and served from there afterwards, but a region
  never visited online will not appear offline. `--no-map` disables it entirely.
- **The scrub bar reads in percent, not time.** Position is byte-based, which
  only maps to log time if the sentence rate is uniform.
- **Terminal-only omissions**, all deliberate: no polar sky view (the satellite
  table carries the same figures), no map zoom or pan, and no runtime source
  picker — sources are chosen with command-line flags.
- **Trip statistics exclude dead-reckoned fixes** (GGA quality 6) from the track
  and totals. They are still counted and shown in the raw view.

## Attribution and licensing

Waypoint is MIT licensed. The terminal basemap draws vector tiles from
[OpenFreeMap](https://openfreemap.org), built from
[OpenStreetMap](https://www.openstreetmap.org/copyright) data via OpenMapTiles;
the GUI's map tiles come from OpenStreetMap. Attribution is displayed whenever
tiles are on screen, as those licences require. Both are free services — be
considerate of them, and prefer the cache.

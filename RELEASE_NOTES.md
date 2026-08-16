# Waypoint v0.1.1

A documentation release. **No functional change** — if v0.1.0 is working for
you, there is no reason to upgrade.

## What changed

Each crate now carries its own README, so the crates.io pages show what the
package is instead of a bare description. The root README cannot serve that
purpose: cargo packages only files inside a crate's own directory, so each
needs its own.

They are written for the reader each package actually has:

- [`gn183-waypoint-core`](https://crates.io/crates/gn183-waypoint-core) — an
  example, and the two design decisions an integrator would otherwise have to
  infer: that live and recorded sources are deliberately distinguished, and why
  the `nmea` crate's stateful parser is not used.
- [`gn183-waypoint-tui`](https://crates.io/crates/gn183-waypoint-tui) — install,
  what appears on screen, and the full key list.
- [`gn183-waypoint-gui`](https://crates.io/crates/gn183-waypoint-gui) — install,
  the sky view and map behaviour, and how live sources differ from recordings.

One point worth repeating from the library's README, because it looks like a
mistake and is not: the package is `gn183-waypoint-core`, but the library it
exposes is `waypoint_core`, so that is what `use` refers to. The package name
had to be unique on crates.io; the library did not.

## Installing

Prebuilt binaries for Linux, macOS and Windows are attached to this release.
Unpack and run — both frontends are in the archive.

```sh
cargo install gn183-waypoint-tui   # terminal dashboard
cargo install gn183-waypoint-gui   # map dashboard
```

## What Waypoint is

An NMEA 0183 debugger that reads from a serial port, a TCP endpoint or a
recorded log, and shows what the receiver is actually doing — on a map, and in a
terminal. It keeps what a tracker throws away: the raw sentences, the checksum
failures, the parse errors, the satellites in view but unused.

The full feature list, design notes and known limitations are in the
[v0.1.0 notes](docs/release/v0.1.0.md), which still apply. In particular:

- **Serial has still not been exercised against real hardware.** TCP has,
  against [tab5-gps-monitor](https://github.com/arunkumar-mourougappane/tab5-gps-monitor)
  on port 10110, including recording.
- A recording is flushed about once a second, so a *killed* process can lose up
  to a second of capture. Quitting normally flushes everything.
- `cargo test` does not run from the published `gn183-waypoint-tui` tarball; one
  test reads the shared sample log from the repository root. Building and
  installing are unaffected.

## Licensing

MIT. Map data from OpenStreetMap, and the terminal basemap's vector tiles from
OpenFreeMap via OpenMapTiles; attribution is shown whenever tiles are on screen.

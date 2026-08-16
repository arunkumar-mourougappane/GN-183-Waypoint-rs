# Overview

## Problem

GPS/GNSS modules speak NMEA 0183, a line-based, comma-separated sentence format,
over serial, or over TCP when bridged by a network-attached receiver or SDR. There's
no single Rust tool to ingest that stream from any of those transports (or a
recorded log file), decode it, and give a live, at-a-glance picture of what the
receiver is doing: is it holding a fix, how good is that fix, where has the vehicle
been, how fast is it moving, and which satellites it's actually using.

Waypoint is that tool: an NMEA debugger and trip visualizer with a live map/track
view and a parameter dashboard, driven from serial, file replay, or TCP.

## Where it came from

Waypoint was written to test and drive
[tab5-gps-monitor](https://github.com/arunkumar-mourougappane/tab5-gps-monitor),
an M5Stack Tab5 (ESP32-P4) application that reads an AT6668 GPS/BDS unit. That
device emits NMEA over UART, serves it over a Wi-Fi AP on TCP 10110, and logs it
to SD — which is exactly why this reads serial, TCP and files, and why the
live-versus-recorded distinction runs through the whole design. The three
sources are not a generality exercise; they are one device's three outputs.

## Goals

- Ingest NMEA 0183 from serial port, TCP socket, or a static/replayed log file
  through one common interface.
- Parse the sentence set needed for positioning and quality metrics: GGA, RMC, GSA,
  GSV, VTG, GLL, GST, ZDA.
- Plot the live track on a map, with a current-position/heading marker.
- Surface fix type (no fix / 2D / 3D), speed, altitude, course/heading, HDOP, and
  satellite count broken out by constellation (GPS/GLONASS/Galileo/BeiDou/QZSS).
- Compute trip statistics: distance traveled, elapsed vs. moving time, avg/max
  speed, elevation gain/loss.
- Offer two frontends against the same core: a native GUI (map + gauges) and a
  terminal UI (for headless/SSH/embedded use). Both show the same figures, and
  both put the track against real geography — the GUI on map tiles, the terminal
  on vector road and water geometry drawn in braille.
- Treat a live receiver and a recorded log as the different things they are: one
  can be paused, retimed and scrubbed; the other can fall silent and has to
  acquire a fix.

## What it is not

The distinction worth stating early, because it shapes everything: this is a
**debugger**, not a tracker. It exists to answer "what is this receiver doing
and why", so it keeps what a tracker would throw away — the raw sentences, the
checksum failures, the parse errors, the satellites in view but unused, the
dead-reckoned fix that should not count toward distance. Anything that hides bad
data to make the display tidier is working against the purpose.

## Non-goals (for now)

- Not a full chartplotter or marine navigation suite (no route planning, no AIS).
- Not a GNSS receiver configuration tool (no sending of proprietary config
  sentences back to the device) — read-only debugging first.
- No mobile app; desktop/terminal only.
- No cloud sync or multi-user session sharing.

## Glossary

| Term | Meaning |
|---|---|
| NMEA 0183 | Line-based ASCII sentence protocol used by marine/GNSS electronics |
| Talker ID | 2-letter prefix identifying the sentence source/constellation (e.g. `GP`, `GN`) |
| Fix type | Whether the receiver has no fix, a 2D fix (lat/lon only), or 3D fix (+altitude) |
| Fix quality | GGA's finer-grained indicator: invalid, GPS, DGPS, RTK fixed/float, etc. |
| HDOP / VDOP / PDOP | Horizontal/Vertical/Position Dilution of Precision — lower is better geometry |
| GSV | Satellites-in-view sentence: per-satellite PRN, elevation, azimuth, SNR |
| COG | Course Over Ground — direction of travel, not necessarily where the bow points |
| Checksum | XOR of all bytes between `$`/`!` and `*`, used to validate sentence integrity |
| TTFF | Time to first fix: how long a receiver took to acquire from a cold start |
| MVT | Mapbox Vector Tile — map geometry as coordinates rather than imagery, which is what makes a basemap possible in a terminal |

## Where to read next

| | |
|---|---|
| [`01-nmea-protocol.md`](01-nmea-protocol.md) | The sentences consumed, field by field |
| [`02-data-sources.md`](02-data-sources.md) | Serial/TCP/file ingestion, and the live-versus-recorded split the UI branches on |
| [`03-architecture.md`](03-architecture.md) | Workspace layout, `Engine`, concurrency |
| [`04-crate-selection.md`](04-crate-selection.md) | Every dependency, why it is there, and what was rejected |
| [`05-ui-gui.md`](05-ui-gui.md) / [`06-ui-tui.md`](06-ui-tui.md) | What each frontend shows, and where they deliberately differ |
| [`07-trip-statistics.md`](07-trip-statistics.md) | How each figure is computed, and the guards against bad fixes |
| [`08-roadmap.md`](08-roadmap.md) | What is built, what is open, what is deferred |
| [`09-testing.md`](09-testing.md) | How this is verified, and what past bugs taught about fixtures |

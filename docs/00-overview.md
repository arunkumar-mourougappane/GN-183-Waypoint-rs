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
  terminal UI (for headless/SSH/embedded use).

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

See [`01-nmea-protocol.md`](01-nmea-protocol.md) for the full sentence reference,
[`03-architecture.md`](03-architecture.md) for how the pieces fit together, and
[`08-roadmap.md`](08-roadmap.md) for build order.

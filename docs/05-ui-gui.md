# GUI Frontend (egui)

`waypoint-gui`: an `eframe`/`egui` app reading `GnssState` from `waypoint-core`
(see [`03-architecture.md`](03-architecture.md)).

## Layout

```
┌─────────────────────────────────────────────┬───────────────────┐
│                                               │  Fix: 3D  ●        │
│                                               │  Sats: 14 (GPS 6,  │
│                                               │   GLO 4, GAL 4)    │
│              walkers map panel               │  HDOP: 0.9         │
│         (live track + heading marker)        ├───────────────────┤
│                                               │  Speed   42.3 kt   │
│                                               │  Alt    112.4 m    │
│                                               │  COG     271°      │
├───────────────────────────────────────────────┤  Heading  268°     │
│  speed / altitude / HDOP time-series (egui_plot)│                  │
├───────────────────────────────────────────────┼───────────────────┤
│         satellite sky-view (polar)             │   Trip stats      │
└───────────────────────────────────────────────┴───────────────────┘
```

Exact arrangement is a detail to iterate on; the panel breakdown below is what
matters.

## Map panel (`walkers`)

- Track rendered as a polyline from `GnssState`'s accumulated `TrackPoint`s.
- Current-position marker rotated to heading/COG.
- Auto-follow toggle: recenter on latest fix vs. free pan/zoom (user panning
  should disable auto-follow until re-enabled, standard slippy-map UX).
- Tile source configurable: default to an OSM-compatible XYZ endpoint; support
  offline `.pmtiles` for field use with no connectivity (walkers supports this
  natively — see [`04-crate-selection.md`](04-crate-selection.md)).
- No-fix state: show last-known position dimmed/frozen with a "no fix" overlay
  rather than clearing the map.

## Status panel

- **Fix type badge**: No Fix / 2D / 3D, colored (e.g. red/yellow/green), sourced
  from GSA (see [`01-nmea-protocol.md`](01-nmea-protocol.md)); GGA's finer fix
  quality (DGPS/RTK/etc.) as a secondary label when present.
- **Satellite count**, broken out by constellation (talker ID), not just a total.
- **HDOP** (and PDOP/VDOP if space allows), numeric + a simple quality band
  (e.g. <1 excellent, 1-2 good, 2-5 moderate, >5 poor — standard DOP
  interpretation bands).

## Time-series (`egui_plot`)

- Rolling-window line plots for speed, altitude, and HDOP over the session (or
  last N minutes, configurable), each on its own small plot rather than one
  crowded multi-axis chart.
- Shared X-axis (time) across the three so a spike in one is easy to correlate
  with the others.

## Satellite sky-view

- Polar scatter plot (`egui_plot`, custom polar transform or plotted in
  cartesian with elevation mapped to radius): each tracked satellite placed by
  azimuth (angle) and elevation (radius, center = zenith/90°, edge = horizon/0°),
  colored by constellation, sized/shaded by SNR.
- Reassembled from the full GSV group per epoch (see
  [`01-nmea-protocol.md`](01-nmea-protocol.md)) — don't redraw mid-reassembly.
- Below it, a per-satellite signal-strength bar chart (filled = used in the fix,
  hollow = in view only), plus per-constellation `used/in-view` counts in the
  status panel.

## Trip stats panel

See [`07-trip-statistics.md`](07-trip-statistics.md) for the metrics; render as a
simple label/value list (distance, elapsed, moving time, avg/max speed, elevation
gain/loss) with a reset-trip action.

## Transport and liveness

The top bar adapts to what the source can do (see
[`02-data-sources.md`](02-data-sources.md)):

- **Replay**: play/pause, restart, ½×/2× rate stepping with the current
  multiplier, and a progress bar showing position in the log.
- **Live**: sentences-per-second, replaced by a "no data for Ns" warning once
  the stream goes quiet, plus time-to-first-fix and data rate in the status
  panel.
- **Instant load**: neither — the log is parsed and done.

## Source control

- A "Source…" panel for selecting source type (serial/TCP/file) and its
  parameters, matching the CLI flags in the [README](../README.md) so the same
  session can be started either from the command line or adjusted live in the
  GUI. Serial ports come from a rescannable dropdown; log files are chosen
  through a native file dialog (`rfd`), not a typed path.
- Connection-status indicator reflecting `waypoint-core`'s reconnect/error state
  (see [`03-architecture.md`](03-architecture.md)).

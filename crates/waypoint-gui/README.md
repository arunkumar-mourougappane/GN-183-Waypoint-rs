# gn183-waypoint-gui

Map dashboard for [Waypoint](https://github.com/arunkumar-mourougappane/GN-183-Waypoint-rs),
an NMEA 0183 debugger. Reads a GPS receiver over serial or TCP, or a recorded
log, and plots the track on OpenStreetMap tiles alongside the numbers that say
whether to believe it.

```sh
cargo install gn183-waypoint-gui
waypoint-gui --file drive.nmea          # replays in its original timing
waypoint-gui --tcp 192.168.4.1:10110    # a live receiver
waypoint-gui                            # then pick a source in the UI
```

## What it shows

- **The live track on a slippy map**, with a heading marker, follow-the-fix, and
  the trail decimated to one vertex per two screen pixels so a long session
  stays cheap to draw.
- **A polar sky view** — every satellite placed by azimuth and elevation,
  coloured by signal strength, filled when it is used in the fix and hollow when
  it is merely in view. Plus per-satellite signal bars.
- **Fix and trip panels** — 2D/3D mode, fix quality, the DOP triple with quality
  bands, position, altitude, speed, course, and trip statistics guarded against
  fixes that imply impossible speeds.
- **Time-series plots** for speed, altitude and HDOP on a shared axis.
- **The raw sentence stream**, tagged with what the parser made of each line.

## Live sources and recordings differ

A recording gets a transport: play/pause, rate stepping, restart, and a scrub
bar that seeks rather than merely reporting — it works while paused, which is
when scrubbing is most useful.

A receiver gets liveness instead: sentences per second, a warning when the
stream goes quiet, time to first fix, and a record button that captures the
session to a log the file source replays.

Neither is offered what it cannot honour.

## Source picker

Serial ports come from a rescannable dropdown, TCP takes a `host:port`, and logs
are chosen through a native file dialog. Everything available on the command
line is adjustable while running.

## See also

[`gn183-waypoint-tui`](https://crates.io/crates/gn183-waypoint-tui) for the
terminal dashboard, and [`gn183-waypoint-core`](https://crates.io/crates/gn183-waypoint-core)
for the library both are built on.

## License

MIT. Map tiles from OpenStreetMap; attribution is shown on the map.

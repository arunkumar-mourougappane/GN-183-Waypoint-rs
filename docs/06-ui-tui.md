# TUI Frontend (ratatui)

`waypoint-tui`: a `ratatui` app reading the same `GnssState` as the GUI (see
[`03-architecture.md`](03-architecture.md)), for headless/SSH/embedded debugging
where a windowing system isn't available or wanted.

## Layout

```
┌─ Track ───────────────────────┬─ Status ─────────┐
│                                │ Fix: 3D   HDOP 0.9 │
│   braille-canvas track plot   │ Sats: 14 (GP6 GL4  │
│   (Canvas widget, no tiles)   │       GA4)          │
│                                │ Spd 42.3kt Alt 112m │
│                                │ COG 271°  Hdg 268°  │
├────────────────────────────────┴───────────────────┤
│  speed   ▁▂▃▅▇▆▄▃▂▁  sparkline                       │
│  alt     ▂▂▃▃▄▄▅▅▆▆  sparkline                       │
│  hdop    ▁▁▁▂▁▁▂▁▁▁  sparkline                       │
├──────────────────────────────────────────────────────┤
│  Satellite table (PRN, constellation, elev, az, SNR)  │
├──────────────────────────────────────────────────────┤
│  Trip: 12.4 km · moving 00:38:12 · avg 19.4kt         │
└──────────────────────────────────────────────────────┘
```

## Track panel

- `ratatui::widgets::canvas::Canvas` with a `Points`/`Line` shape plotting
  `TrackPoint`s in lat/lon space, auto-scaled to the track's bounding box (no
  real map tiles — terminals can't render raster/vector tiles). This is the TUI's
  answer to the GUI's `walkers` map: same data, ASCII/braille rendering instead.
- Current position marked distinctly (different color/glyph) from the trail.
- No-fix state: same "last known position, dimmed" treatment as the GUI.

## Status block

Same fields as the GUI's status panel (fix type, per-constellation satellite
count, HDOP, speed, altitude, COG, heading) — plain text/labeled values, colored
via ratatui `Style` (fix-type badge color-coded the same red/yellow/green scheme
as the GUI for consistency).

## Sparklines

`ratatui::widgets::Sparkline` for speed, altitude, HDOP — the terminal-appropriate
equivalent of the GUI's `egui_plot` time-series. Rolling window sized to terminal
width rather than a fixed time range.

## Satellite table

`ratatui::widgets::Table`: one row per tracked satellite (PRN, constellation from
talker ID, elevation, azimuth, SNR), sortable/filterable by constellation if input
handling allows; this substitutes for the GUI's polar sky-view, which doesn't
translate well to a text grid.

## Trip stats

Panel alongside the sparklines: distance, elapsed/moving/stopped, average and
max speed, elevation gain/loss, average HDOP, and the rejected-jump count.

See [`07-trip-statistics.md`](07-trip-statistics.md) for how each value is
computed. It sits beside the sparklines rather than in a status line because at
40 columns the full set is more useful than a compact summary, and the space is
already there.

## Input handling

- `crossterm` event loop (via ratatui) for keybindings: `q`/`Esc` quit, `r`
  reset trip, `n` toggle the lower panel between the parameter sparklines and
  the raw sentence log.
- The loop selects between `Engine::updates()` and a 50 ms tick, so it redraws
  when data arrives and still polls input promptly on a silent stream.
- Because logs would corrupt the alternate screen, tracing output is only
  enabled when `WAYPOINT_LOG` names a file.

## Why not reuse GUI panel code

`waypoint-gui` and `waypoint-tui` render the same `GnssState` but through
unrelated widget toolkits (`egui_plot`/`walkers` vs. `ratatui` canvas/sparkline);
there's no meaningful code to share below the `waypoint-core` boundary, so each
frontend owns its own rendering logic entirely. Keeping the panel *breakdown*
(map/track, status, time-series, satellites, trip stats) consistent between the
two docs is what keeps the two UIs feeling like the same tool.

# gn183-waypoint-tui

Terminal dashboard for [Waypoint](https://github.com/arunkumar-mourougappane/GN-183-Waypoint-rs),
an NMEA 0183 debugger. Reads a GPS receiver over serial or TCP, or a recorded
log, and shows what it is actually doing — over SSH, on any terminal.

```sh
cargo install gn183-waypoint-tui
waypoint-tui --file drive.nmea          # replays in its original timing
waypoint-tui --tcp 192.168.4.1:10110    # a live receiver
waypoint-tui --serial /dev/ttyUSB0 --baud 115200
```

## What it shows

```
┌ Track (298 points) · map online ─────────────────────┐┌ Fix ──────────────────┐
│⣆     ⠱⡀     ⠑⢄     ⠈⢆         ⠘⢼        ⡸      ⢸    ⡏ ││Position 51.477693, …  │
│  ⣧     ⠈⠢⡀⠑⡄   ⠈⢆    ⠈⠑⠤⡀      ⠈⢆      ⡠⠊  ⢀⡤⡀ ⡠⠊   ⢰ ││HDOP     0.70 (Ideal)  │
└─────────────────────── © OpenMapTiles © OpenStreetMap ┘└───────────────────────┘
```

- **The track on a real basemap.** Roads, water and green space are decoded from
  vector tiles and stroked onto the same braille canvas as the track — geometry,
  not imagery, so it works on any terminal. Tiles come from the network when it
  is reachable and from a platform cache when it is not. `--no-map` turns it off
  and makes no network requests.
- **Fix detail** — 2D/3D mode, fix quality, DOP with quality bands, satellites
  by constellation with used-in-fix marked, and a satellite table with SNR bars.
- **Parameter sparklines** for speed, altitude and HDOP, and a trip panel.
- **The raw sentence stream**, tagged with what the parser made of each line.
  This is a debugger: checksum failures and parse errors are kept, not hidden.

## Keys

`q` quit · `r` reset trip · `n` toggle plots/raw sentences

Replaying a log: `space` pause · `←`/`→` scrub ±5% · `Home` jump to the start ·
`-`/`+` halve/double the rate · `1` real time · `R` restart.

On a live source: `w` starts and stops recording to a timestamped log that
replays — including the damaged sentences, since those are usually the reason
for recording.

Logs would corrupt the display, so they are written only when `WAYPOINT_LOG`
names a file.

## See also

[`gn183-waypoint-gui`](https://crates.io/crates/gn183-waypoint-gui) for the map
dashboard, and [`gn183-waypoint-core`](https://crates.io/crates/gn183-waypoint-core)
for the library both are built on.

## License

MIT. Basemap tiles from OpenFreeMap, built from OpenStreetMap data via
OpenMapTiles; attribution is shown whenever tiles are on screen.

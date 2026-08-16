# NMEA 0183 Protocol Reference

Scope: the sentence types Waypoint needs to consume. This is not a full NMEA 0183
spec reproduction — only the fields the dashboard/map/trip-stats engine reads.

## Sentence framing

```
$GNGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47
^        ^                                                  ^  ^
|        payload fields (comma separated)                   |  checksum (hex, XOR of bytes between $ and *)
talker ID + sentence ID                                 empty field
```

- Starts with `$` (or `!` for AIS, not used here).
- Talker ID (2 chars) + sentence ID (3 chars), e.g. `GN` + `GGA`.
- Fields are comma-separated; empty fields are valid (missing data, not an error).
- Terminated by `*`, a 2-hex-digit checksum, and `\r\n`.
- Checksum = XOR of every byte between `$` and `*`. Reject/flag sentences that fail
  this check rather than parsing partial data.

## Talker ID → constellation

| Talker ID | Constellation |
|---|---|
| `GP` | GPS (USA) |
| `GL` | GLONASS (Russia) |
| `GA` | Galileo (EU) |
| `GB` / `BD` | BeiDou (China) |
| `GQ` | QZSS (Japan) |
| `GI` | NavIC (India) |
| `GN` | Combined/multi-constellation solution (most modern receivers default to this) |

Satellite-count-by-type in the dashboard should bucket GSV sentences by talker ID,
not assume a single constellation.

## Sentences consumed

### GGA — Global Positioning System Fix Data
Position + fix quality snapshot. One GGA per fix epoch.

| Field | Meaning |
|---|---|
| Time | UTC time of fix, `hhmmss.ss` |
| Latitude, N/S | Position |
| Longitude, E/W | Position |
| **Fix quality** | `0`=invalid, `1`=GPS fix, `2`=DGPS fix, `3`=PPS fix, `4`=RTK fixed, `5`=RTK float, `6`=estimated (dead reckoning), `7`=manual input, `8`=simulation |
| Satellites used | Count of satellites used in the fix (not in-view) |
| **HDOP** | Horizontal dilution of precision |
| Altitude, units | Altitude above mean sea level (usually meters) |
| Geoid separation, units | Difference between WGS84 ellipsoid and MSL |

Fix quality is the primary "is this fix any good" signal and should drive the
fix-type indicator alongside GSA.

### RMC — Recommended Minimum Navigation Information
The "everything you need" sentence — most receivers emit it every epoch alongside GGA.

| Field | Meaning |
|---|---|
| Time, Date | UTC time + date, combine for a full timestamp |
| Status | `A`=active/valid, `V`=void — gate all downstream processing on this |
| Latitude/Longitude | Position |
| **Speed over ground** | In knots |
| **Course over ground** | Track angle, degrees true |
| Magnetic variation | Optional, degrees + E/W |

RMC is the primary source for speed and course; combine with GGA for altitude/HDOP
since RMC has neither.

### GSA — GNSS DOP and Active Satellites

| Field | Meaning |
|---|---|
| Mode | `M`=manual 2D/3D, `A`=automatic |
| **Fix type** | `1`=no fix, `2`=2D fix, `3`=3D fix |
| Satellite IDs (up to 12) | PRNs used in this fix |
| PDOP, **HDOP**, VDOP | Position/Horizontal/Vertical dilution of precision |

Fix type here is what should drive a "No Fix / 2D / 3D" status badge — GGA's fix
quality is a richer signal but GSA's fix type is the canonical 2D/3D distinction.
Multi-constellation receivers emit one GSA per constellation (`GPGSA`, `GLGSA`, …);
the dashboard should treat the combined/last-seen GSA as the current fix type, or
show a GSA per constellation if the receiver reports them separately.

### GSV — Satellites in View
Emitted in multiple sentences per epoch (message N of M) since one sentence holds
up to 4 satellites.

| Field | Meaning |
|---|---|
| Total messages, message number | For reassembling the multi-sentence group |
| **Satellites in view** | Total count (repeated in every message of the group) |
| Per-satellite block ×4: PRN, elevation (0-90°), azimuth (0-359°), SNR (0-99 dB, blank if not tracked) | Feeds the sky-view plot |

Reassemble the full satellite list per talker ID per epoch before updating the
sky-view/count widgets, otherwise the UI flickers between partial sets.

### VTG — Course Over Ground and Ground Speed

| Field | Meaning |
|---|---|
| Course, true | Degrees |
| Course, magnetic | Degrees |
| **Speed** | Knots, and again in km/h |

Redundant with RMC's speed/course fields on most receivers; useful as a
cross-check or fallback when RMC is absent.

### GLL — Geographic Position, Latitude/Longitude
Bare position + time + validity (`A`/`V`). Fallback position source when GGA/RMC
aren't present; rarely needed if GGA and RMC are already parsed.

### GST — GNSS Pseudorange Error Statistics
Per-axis error estimates (lat/lon/alt standard deviation in meters). Optional —
not all receivers emit it — but if present it's a better accuracy indicator than
HDOP alone and worth surfacing in an "advanced" panel.

### ZDA — Time and Date
UTC time + full date (day/month/year) + local zone offset. Useful when RMC's date
field is absent or when precise UTC is needed independent of a fix.

## Rust parsing

The [`nmea`](https://crates.io/crates/nmea) crate covers all of the sentences
above plus GNS, HDT, TXT and others, and exposes a `Satellite` struct (PRN, SNR,
elevation, azimuth) for GSV data.

Its *stateless* `parse_str` is what this uses, not the stateful `Nmea` parser.
The stateful one looks like the obvious choice — it merges GGA/RMC/GSA into a
"current fix" for you — but it does not retain GSA's 2D/3D mode (`GsaMode2`) and
does not group GSV by talker, which are exactly the two things the fix badge and
the per-constellation counts need. `Aggregator` in `waypoint-core` does that
bookkeeping instead; see [`04-crate-selection.md`](04-crate-selection.md).

Two behaviours worth knowing, both learned the hard way against real receiver
output:

- A `GN` talker emits one GSA **per constellation** in the same cycle, so a
  repeated talker ID is normal there and must accumulate. Treating it as a new
  cycle leaves only the last constellation marked as used.
- A constellation with nothing in view still reports itself
  (`$GQGSV,1,1,00,1*65`), and satellites in view but untracked carry an empty
  SNR field — which is not the same as a measured zero.

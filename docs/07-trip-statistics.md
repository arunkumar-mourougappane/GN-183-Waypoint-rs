# Trip Statistics

Computed incrementally in `waypoint-core` as new fixes arrive (see
[`03-architecture.md`](03-architecture.md)) rather than recomputed from the full
track on every update — each new `TrackPoint` updates running totals in O(1).

## Metrics

| Metric | Computation |
|---|---|
| **Distance traveled** | Cumulative sum of haversine distance between consecutive fixes, via `haversine_distance_m` in `trip.rs`. Pairs with no fix, or an implausible jump (below), are skipped. |
| **Elapsed time** | Timestamp of latest fix minus timestamp of first fix. |
| **Moving time** | Sum of inter-fix intervals where instantaneous speed exceeds a threshold (e.g. 0.5-1 kt, to absorb GPS jitter while stationary — exact threshold should be user-configurable since it's receiver/noise dependent). |
| **Stopped time** | `elapsed - moving`. |
| **Average speed** | Distance / moving time (moving average, more meaningful than distance/elapsed for anything with stops) — report both if there's room. |
| **Max speed** | Max instantaneous speed (from RMC/VTG) seen over the trip. Not separately outlier-filtered: the jump rejection below already discards the epochs where a spurious speed would come from. |
| **Elevation gain / loss** | Sum of positive / negative altitude change, measured against a *moving reference* rather than the previous sample. GGA altitude is noisy (±5-10 m even with a good fix), so a delta is only taken once it clears a noise floor, at which point the reference moves to the new altitude. |
| **Avg HDOP / avg satellite count** | Running mean over the trip — a quick "how good was reception overall" summary distinct from the live instantaneous values shown in the status panel. |

### Why the reference moves

Comparing each sample to the one before it looks equivalent and is not. A steady
climb of 0.3 m per fix never produces a single delta above a 1 m floor, so every
step is discarded and a real ascent reports as zero — which is exactly what the
first implementation here did, reading `+0 m` up a hill. Holding a reference and
moving it only when the floor is cleared accumulates the same climb correctly,
while still rejecting jitter around a fixed altitude. Both behaviours are pinned
by tests in `trip.rs`.

## Data quality guards

- **Gate on fix validity**: only fold a fix into trip stats when RMC status is
  `A` (active) and/or GSA fix type is 2D/3D — a `V`/no-fix epoch shouldn't move
  the distance/speed totals.
- **Jump rejection**: a implausible position jump between consecutive valid
  fixes (e.g. implying a speed far beyond anything the vehicle/receiver could
  produce) is almost always a bad fix, not real movement — exclude it from
  distance/speed accumulation, but still record it in the track for debugging
  visibility (this is a debugger, hiding bad data from the map isn't the goal —
  only from the derived stats).
- **First-fix baseline**: trip stats start accumulating from the first valid fix
  of the session, not from process start — a source that takes a while to get a
  fix shouldn't count that dead time as "stopped" trip time.

## Reset semantics

Both frontends expose a "reset trip" action (see
[`05-ui-gui.md`](05-ui-gui.md), [`06-ui-tui.md`](06-ui-tui.md)) that zeroes the
running totals and clears the track, without touching the live source
connection — useful for timing a specific leg of a longer session without
restarting the app.

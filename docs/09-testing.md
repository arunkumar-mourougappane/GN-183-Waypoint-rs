# Testing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

CI runs those three on Linux, macOS and Windows — see
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml).

## What is covered where

| Layer | How |
|---|---|
| Parsing, state, trip statistics | Unit tests in `waypoint-core`, against inline sentences |
| Source behaviour | Timing tests driving the real file source: that a replay is paced, that instant load is not, that rate changes and seeks take effect mid-replay |
| Dashboard layout | The whole TUI rendered through ratatui's `TestBackend` and asserted as text — no terminal needed |
| Key bindings | The `Action` mapping is extracted from the event loop precisely so it can be tested |
| CLI defaults | What each invocation resolves to, because a default is not visible anywhere else |

## What hardware has confirmed

TCP is verified against a real receiver —
[tab5-gps-monitor](https://github.com/arunkumar-mourougappane/tab5-gps-monitor)
serving NMEA over its access point on port 10110 — including capturing a session
to a log. Everything else here is tested against files, sockets and fixtures,
which is worth remembering when reading the rest of this page: the serial path
in particular has never met a UART.

## A dependency that cannot be updated

`Cargo.lock` pins `serde_with 3.11.0`, against which there is a security
advisory (fixed in 3.21.0). It is ignored in `.github/dependabot.yml`, for two
reasons worth stating rather than burying:

- It is **not compiled**. `serde_with` is an optional dependency of `nmea`,
  behind a `serde` feature this workspace does not enable. `Cargo.lock` records
  dependencies irrespective of feature activation, so it appears there while
  having no dependency edge and producing no build artifact.
- It **cannot be updated anyway**. `nmea 0.8.0` requires `~3.11` — that is,
  `>= 3.11.0, < 3.12.0` — so no version satisfying the advisory can resolve.
  Dependabot retries on every push and fails with `update_not_possible`.

Both conditions have to hold for the entry to stay ignored. If this workspace
ever enables nmea's `serde` feature, the advisory becomes real and the ignore
must go.

## A packaging caveat

One TUI test includes `assets/sample.nmea` from the repository root, which is
outside that crate's package directory. Publishing and installing are unaffected
— `cargo publish` verifies by building, not testing — but `cargo test` will not
compile from the published tarball. Running the tests means running them from
the repository, which is where anyone changing this code already is.

## Lessons the bugs taught

Each of these cost real debugging time and is worth not repeating.

### A tidy fixture hides the bugs that matter

The sample log originally ticked on whole seconds. Whole-second gaps divide
exactly by any replay rate, so the arithmetic always landed on zero — and a
replay that could not terminate survived forty tests and four hands-on runs. A
real receiver's timestamps drift off the second; the first fractional gap froze
the replay permanently.

`assets/sample.nmea` is now deliberately awkward: fractional gaps, five GSA
sentences per epoch under one talker, a constellation reporting nothing in view,
untracked satellites with no SNR, a dead-reckoned fix, damaged sentences.
`assets/generate_sample.py` documents why each one is there.

The corollary: **a fixture generated here cannot prove correctness**, only that
real shapes are handled. It agrees with our reading of the spec by construction.
Checking decoded positions against a track published with a real recording is
the only thing that catches a coordinate conversion that is subtly wrong.

There is a ready-made source for that:
[tab5-gps-monitor](https://github.com/arunkumar-mourougappane/tab5-gps-monitor)
writes **both** raw NMEA and its own decoded track points to SD, as
`gps_NNNN.nmea` and `track_NNNN.csv`. Comparing the two independently verifies
the degrees-minutes conversion and hemisphere handling. A trial run against one
such pair matched to 1e-6 degrees — about 11 cm — across every epoch, and turned
up a one-epoch lag in the *reference* CSV's altitude, HDOP and satellite
columns rather than an error here. Committing such a fixture means publishing a
real route, so it needs a recording whose location is not sensitive.

### Verify the screen, not the byte stream

ratatui renders *differentially*, writing only the cells that changed. Grepping
a terminal's output for `×4` finds nothing, because it emits a bare `4` after a
cursor move. A probe built that way reported every rate and seek key as dead
when all of them worked.

To check a TUI end to end, reconstruct the screen from the escape sequences
(cursor addressing plus printable text) and assert on *that*. Note also that a
pty needs its window size set explicitly — a zero-sized terminal renders nothing
at all and looks identical to a crash.

### `%.2f` is not a chrono specifier

`chrono` accepts `%.3f`, `%.6f`, `%.9f`. `%.2f` makes `Display` return `Err`,
and `to_string` panics on that. It took down the GUI's first run, then was
reintroduced in the terminal months later. Both frontends now use `%.3f`, and
the render tests catch it before a terminal ever sees it.

### The two frontends drift apart unless something checks

Both render the same `GnssState`, and the terminal quietly went five figures
short — geoid separation, the fix date, average overall speed, average
satellites, the fix count. A missing readout is invisible until someone goes
looking for it, so a test now asserts each field is on screen.

### Environment can lie about the filesystem

Clearing the tile cache from a sandboxed shell appeared to succeed — the shell
saw it gone — while the application still read the old file. Time was lost
suspecting the cache logic, which was correct. When a filesystem check
contradicts the code, drive the component directly (a small example binary
against the real API) before assuming the code is wrong.

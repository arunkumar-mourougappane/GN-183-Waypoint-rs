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
the only thing that catches a coordinate conversion that is subtly wrong — see
the open item in [`08-roadmap.md`](08-roadmap.md).

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

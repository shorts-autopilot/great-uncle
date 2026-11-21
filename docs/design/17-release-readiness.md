# 17 · Release readiness

**Rank** 17 · **Order** 17 · **Size** L · **No format change** · **Depends on** everything before it

## Problem

The project is a prototype with production ambitions and none of the
scaffolding: no CI, no fuzzing on the three parsers that take untrusted input,
no hardware probe, no packaging, and a Linux-only serial layer with no stated
support matrix. Its largest untested surface is not the command line — it is the
`.gdtf`/`.mvr` archive reader, the show loader and the timecode parser, all of
which consume files from outside the project.

Nothing here adds a feature. It is the work that makes the previous sixteen
items safe to ship.

## Behaviour when this is done

```
openlight --probe
  /dev/serial/by-id/usb-FTDI_FT232R_USB_UART_A50285BI-if00-port0  generic
    3 frames in 92 ms (32.6 Hz), break 176 µs, MAB 12 µs, jitter 1.8 ms
  /dev/ttyACM0  unknown (no DMX framing observed)
```

```
$ cargo run -p openlight-console -- --check "broken.olshow"
broken.olshow: fixture 3 overlaps fixture 4 on universe 1 (10..13 and 12..15)
$ echo $?
1
```

## Design

### Support matrix

| Platform | Output | Status |
|---|---|---|
| Linux | generic serial, ENTTEC, uDMX | supported, primary |
| macOS | generic serial, ENTTEC | supported; compile-checked, hardware-gated |
| Windows | generic serial, ENTTEC | built in item 23; COM is unit-tested on Windows CI and hardware-gated |
| any | `virtual` | supported everywhere, used by every test |

The `serial.rs` platform modules cover Linux, macOS and, since item 23,
Windows; every other target has a fallback returning `DmxError::Unsupported`,
so the workspace builds everywhere and only *device* support varies. ENTTEC is
the cross-platform path because the widget owns the framing and the link is
ordinary 57 600 8N1.

### CI

- `cargo test --workspace --locked`, `cargo clippy --all-targets -D warnings`,
  `cargo fmt --check`, `python3 tools/spec_status.py --check` on every push;
- a matrix of stable + MSRV (declared in `Cargo.toml` as `rust-version`) on
  Linux, macOS and Windows for the build and the `virtual` demos;
- one job builds `--features usb` (libusb present on Linux only);
- `cargo deny check` for licences (MIT-compatible only) and advisories;
- the phase demos run in CI and their trace output is diffed against expected
  files, so a rendering regression fails a build rather than a rehearsal.

### Untrusted input

- **Property tests** (`proptest` as a dev-dependency) on:
  - `command::lex` + `parse`: never panics, never loops, always returns
    `Ok`/`Err`;
  - `show::from_str`: any byte string yields `Err` or a `Show` that validates;
  - `timecode::parse`/`format`: round-trip within range, total on any string;
  - `Desk`/`Cue` JSON: same rule.
- **Fuzz targets** (`cargo fuzz`, nightly, run for 10 minutes per release, not
  per PR): `Show::load`, `gdtf::import`, `mvr::import`, `command::parse`.
  Corpus seeds are the example files.
- **Archive limits**, enforced before parsing, in one `limits.rs`:
  ≤ 64 entries per archive, ≤ 4 MiB per entry uncompressed, ≤ 16 MiB total,
  no absolute or `..` paths (zip-slip), ≤ 512 channels per mode, ≤ 4096
  functions per channel, ≤ 1024 fixture types per show, ≤ 64 MiB show file
  read. Every limit has a test with an archive that just exceeds it.
- **Show size before parse**: read the metadata length first and refuse > 64 MiB
  with a clear error, so a runaway file cannot exhaust memory before serde sees
  it.

### Hardware probe

`--probe` (and `Probe` as a command, which reports instead of printing):

1. Enumerate candidates: `/dev/serial/by-id/*` on Linux, `/dev/cu.*` on macOS,
   the configured `--output` specs always included.
2. For each, open with the DMX settings, send three frames at 1 Hz, and time
   them with `Instant`. Report achieved rate, break/MAB from the settings, and
   jitter (max − min interval).
3. For a port whose open succeeds but whose writes block, cap at 2 s and report
   `write timed out` — that is the signature of a non-DMX serial device.
4. Never probe a port that is already bound by this process.

This is the only code path that can prove the electrical timing story on real
hardware, and it writes a paste-able report for bug reports.

### Packaging and docs

- `crates/console` gets `[[bin]] name = "openlight"` metadata, a `--version`
  with the git hash (`vergen`-free: `build.rs` writes `OPENLIGHT_GIT_HASH` from
  `git rev-parse --short HEAD` when available, empty otherwise).
- A tarball release with the binary, `README.md`, `docs/`, `examples/` and a
  `packaging/udev/60-openlight.rules` (group `dialout`, `MODE="0660"`) plus a
  short install section.
- `CHANGELOG.md` (Keep a Changelog), `SECURITY.md` (how to report, what the
  desk lock is not), `CONTRIBUTING.md` (the one-writer rule, the test standard
  from `docs/ROADMAP.md`, how to run the demos).
- `docs/PHASE5.md` documenting whatever phase this release is, with the
  limitations list kept honest.

### Performance record

A `benches/render.rs` (criterion as a dev-dependency) measuring:
4096 fixtures × 4 universes, programmer render, playback render with 1000 lanes
(item 02), effect output with 3 targets. Release numbers are recorded in the
docs; there is **no timing assertion in CI** (flaky by nature). The point is a
number an implementer can regression-check by hand, especially for item 02's
lane evaluation.

## Implementation plan

1. Platform fallback in `serial.rs`; macOS `termios` backend; declare support in
   README.
2. `limits.rs` and its wiring into `gdtf`, `mvr`, show load; tests per limit.
3. Property tests for the four parsers; `fuzz/` targets with seed corpus.
4. `--probe` with a pty-based test and a "device does not speak DMX" test.
5. CI workflows, MSRV, `cargo deny`, demo trace diffing.
6. Packaging, udev rules, changelog/security/contributing, version stamping.
7. `benches/render.rs` and a numbers section in `docs/PHASE5.md`.

## Tests

- **limits** — an archive with 65 entries, a 5 MiB entry, a `../` entry, a mode
  with 513 channels: each is refused with a message naming the limit.
- **proptest** — the four properties above, plus a regression corpus for every
  panic the fuzzer finds (the corpus is committed).
- **probe** — a pty that consumes bytes reports the achieved rate; a pty that
  never reads reports `write timed out`; the report is one line per port.
- **ci** — the workflow runs the same commands a developer runs; the demo diff
  fails on a one-byte change to a trace (verified by making one).
- **build** — `cargo check --workspace` for a target without the serial backend
  (a `--target` job or a `cfg` test) succeeds and `generic:` reports
  `unsupported`.

## Boundaries

- No auto-update, no signed MSI, no code signing, no notarisation. Item 23 adds a portable Windows install script.
- No telemetry, no crash reporting service.
- No Windows raw-DMX backend in this item; item 23 adds the COM backend, still hardware-gated.
- No security audit of the remote beyond item 12's design.

## Risks

- **MSRV and dependency drift.** `cargo deny` plus `--locked` in CI keeps it
  honest; the MSRV is tested in the matrix rather than asserted.
- **Fuzzing finds real bugs late.** Budget for it: the archive limits land
  *before* the fuzz job, and every crash becomes a committed regression test, so
  the corpus is the durable output.
- **Hardware support claims.** The probe measures; the docs list exactly what
  was measured on which adapter. Anything else is "reported working by a user",
  labelled as such.

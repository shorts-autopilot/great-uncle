# Changelog

All notable changes to OpenLight are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/) once it reaches 1.0.

## [Unreleased]

### Added

- A colour model: `Color` takes a name, hex, RGB, CMY, HSB or kelvin value and
  writes the selection's own system — additive RGB (with RGBW white
  extraction), subtractive CMY, or the nearest named set on a colour wheel.
  Bare `Color` lists each fixture's system, components and hex, and the sheet
  swatch now reads all three systems. No show or desk format change.
- A native desktop window, `openlight-desktop`: five panes, the command line,
  nine executor faders and a grand master. It is built on `openlight-app`, the
  shared startup and snapshot layer the console now uses too, and is behind the
  non-default `gui` feature. `--headless-smoke` loads and snapshots without a
  display for CI. See `docs/DESKTOP.md`.
- Structured engine listings (`Listings`, sheet/playback/pool/timecode rows and
  status lines) shared by the command line and the window.
- Release readiness: CI on Linux, macOS and Windows, a tested MSRV, archive and
  document limits on every untrusted input, property tests and fuzz targets for
  the show, GDTF, MVR and command parsers, and `--probe` (`Probe`) for measuring
  real DMX framing on a serial port.
- A macOS `termios` serial backend, and a fallback that builds on platforms
  without raw DMX framing.
- Packaging: a release tarball script, udev rules for common USB-serial DMX
  adapters, `SECURITY.md` and `CONTRIBUTING.md`.
- Render benchmarks under `crates/engine/benches/render.rs`.
- `--version` includes the short git hash when the binary was built in a
  checkout.

### Changed

- Show state is read-only outside the engine crate: `Engine::show` is
  `pub(crate)` and `Engine::show()` is the accessor every front end uses.
- The workspace MSRV is 1.89, raised from 1.82 to match the locked dependency
  set. CI tests the declared version in the platform matrix.
- `zip` moved from 2.6 to 9.8 after `cargo deny` found RUSTSEC-2026-0545 and
  RUSTSEC-2026-0548 in the old reader; the archive limits are unchanged.

## [0.1.0] - 2026-11-20

The first seventeen roadmap designs are built. A show is a versioned document
beside a disposable desk file; the desk plays cues, follows external timecode,
moves in black, runs effects against the tempo and can undo. Output has one
self-clocked worker per universe, a journal and autosave survive a power cut,
blackout and lock are reversible, and the console has a scriptable command line,
macros and a terminal shell.

Show format v12, desk format v1.

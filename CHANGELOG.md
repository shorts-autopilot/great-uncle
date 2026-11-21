# Changelog

All notable changes to OpenLight are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/) once it reaches 1.0.

## [Unreleased]

### Added

- A window workspace for the desktop GUI. The window now opens on a blank
  screen and the operator adds, moves, resizes and closes 16 window types on a
  12 × 8 snap grid: Fixture Sheet, Sequence Sheet, Playbacks, Pools, Presets,
  Groups, Worlds, Filters, Effects, Timecode Viewer, Command Line History, DMX
  Sheet, Masters, Running Playbacks, Patch and Status. The Add Window picker
  groups them into Common, Pools, Playback, Tools and All, and the left control
  rail has ADD, CLEAR and RESET. Windows cannot overlap, layouts are session
  state rather than show/desk data, and every widget action still writes command
  text through `App::execute`. Command history is bounded at 100 entries and
  can be recalled into the input. No format change; the hardware/display proof
  remains the release gate.
- File management, backups and templates: saving over an existing show now
  leaves a timestamped show-only backup in a `backups/` folder (ten retained),
  and `Backup ["path"]` takes one explicitly. `Files ["folder"]` lists shows
  with size and UTC modification time; `Save As` writes under a new name;
  `Copy Show`/`Rename Show` move the show and its companion desk as a unit;
  `Delete Show` removes the working pair but keeps recovery files. `Save
  Template`/`Load Template` write and read a show without its desk. No show or
  desk format change; the hardware/display proof remains the release gate.
- Windows serial output: `generic:COM3` and `enttec:COM3` now open a native COM
  handle, configure 250000 8N2 or 57600 8N1, and preserve host-generated
  break/MAB for generic DMX. `--probe` discovers active COM ports from the
  `SERIALCOMM` device map, and a portable ZIP plus per-user PowerShell install
  scripts are included. The backend is cross-compiled and Windows-CI tested;
  no adapter timing has been measured, so physical Windows output remains part
  of the hardware release gate.
- MAtricks, Shuffle and Align: `MAtricks XBlock|XGroup|XWings|XWidth`
  divide a selection into value cells, `MAtricks Transform Mirror` reflects
  them, and `MAtricks Reset` clears the shape. `Shuffle [Seed n|Off]` gives a
  deterministic permutation, and `Align [<attribute>] <from> Thru <to>
  [Transition Linear|Sinus|Slow|Fast]` interpolates across the cells into the
  programmer, including fine-channel resolution. The shape and default
  transition are additive desk fields; show format is unchanged and the
  hardware/display proof remains the release gate.
- DMX input and merge: `--input 1=virtual` or `--input
  1=enttec:/dev/ttyUSB0` reads a universe on its own thread, and
  `Dmx 1 Merge Htp|Ltp|Off`, `Dmx 1 Priority Local|Input`, `Dmx 1 Loss
  Hold|Zero|Fade 2` and `Dmx 1 Timeout 1.5` control a per-universe merge.
  `Dmx`/`Dmx In` report transport, signal, peak, frames and errors; `Dmx 1
  Trigger 10 Sequence 2` fires a cue list on a rising slot edge, and `Dmx 1
  Snapshot` captures the incoming look into the programmer. Show format v14
  adds the universe merge fields; the desk file stays v1. The virtual reader is
  the tested path; the ENTTEC Pro reader is fake-link tested and physical
  input remains part of the hardware release gate.
- Executor pages: `Store Page 2 "Specials"`, `Page 2`, `Page Next`/`Page
  Prev`, `Delete Page 2` and `Assign Page 2 Executor 1 Sequence 3` bank
  executors without stopping playback. Executor numbers and runs are
  page-qualified, so the same number on two pages is two independent cue lists;
  fader, flash and solo handles are per page and the active page is shown by
  `Page`, `Status`, the GUI fader pane and the web remote. Show format v13 adds
  pages and upgrades every older show onto page 1; the desk file stays v1.
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

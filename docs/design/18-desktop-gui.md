# 18 · Desktop GUI

**Rank** 18 · **Order** 18 · **Size** L · **No format change** · **Depends on** 03 (snapshots), 09 (preview), 11 (listings as methods)

## Problem

The terminal shell is a real UI and it is the right first UI, but a desk needs
what a terminal cannot give: several windows visible at once, a fader you can
drag, colour swatches, a cue list you can click, and a screen that a visiting
operator recognises. `README.md` says "there is no desktop GUI yet"; this is the
item that changes that sentence.

The risk is not drawing pixels — it is a second implementation of the engine.
Every design decision here exists to make that impossible.

## Behaviour when this is done

A window with five panes — Sheet, Playback, Pools, Timecode, Status — a command
line at the bottom, a grand master and a blackout button, and the same command
semantics as the terminal. Keyboard-first: the function keys switch panes, the
command line has focus by default, and every action is a command.

## Design

### Crate split

```
crates/app/        new library: wiring shared by every front end
  load(Options) -> App
  App { engine: Arc<Mutex<Engine>>, runtime: Runtime, journal, desk path }
  App::execute(line) -> CommandOutcome
  App::snapshot() -> Snapshot
crates/console/    becomes a thin front end over openlight-app
crates/desktop/    new binary `openlight-desktop`, feature-gated
```

This extraction is the actual work. Today `crates/console/src/main.rs` owns
option parsing, show loading, transport binding, runtime start, script running
and the REPL. The desktop needs all of that except the REPL. `openlight-app`
takes it, the console keeps its argument parser (which is its own product
surface, item 16), and both call `App`.

### Snapshot, not shared mutation

```rust
pub struct Snapshot {
    pub revision: u64,
    pub show_name: String,
    pub listings: Listings,     // sheet rows, playback rows, pool rows, timecode rows, status lines
    pub frame: OutputFrame,     // for level meters / channel grid
    pub desk: DeskView,         // selection, blind, blackout, lock, preview, tempo
    pub playing: Vec<PlaybackView>,
}
```

- The UI thread takes the engine mutex, builds a `Snapshot` (all owned data),
  and releases it. Drawing never holds the lock; a 60 Hz fade cannot be stalled
  by a slow repaint, and the output thread cannot be blocked by the UI.
- `Listings` is produced by the same `Engine` methods item 11 extracted for
  reports. The GUI parses nothing and formats nothing the CLI would format
  differently.
- Input goes the other way: a click or key produces a command **string** and
  calls `App::execute`. Widgets that need a value (a fader drag) produce
  `Executor 1 At 63`, not a direct write. There is no second mutation path.

### Toolkit

Deferred to a one-day spike with these acceptance criteria:

- MIT/Apache-2.0, no GPL, no C++ toolchain requirement beyond what the project
  already needs;
- pure-Rust, cross-platform (Linux/X11+Wayland, macOS, Windows);
- immediate or retained mode both acceptable, but **no async runtime** required
  by the app;
- a software or GL fallback, because show machines have bad drivers;
- text rendering that handles monospace alignment well (the panes are tables);
- a binary-size and dependency budget: the release binary should stay under
  ~20 MB.

Candidates in the spike: `egui`/`eframe` (immediate mode, simplest for a table-
heavy desk), `iced` (retained, Elm-style), `slint` (declarative). The spike
builds the Playback pane only, in each, and measures binary size and startup
time. The design does not commit before that evidence, because "we picked a GUI
framework" is the decision most expensive to reverse.

### Panes

| Pane | Data | Actions |
|---|---|---|
| Sheet | fixture rows with level/colour/position and source (`P`/`Pb`/`H`) | select, `At`, `Home`, `Default` |
| Playback | sequences, cues with trigger/timing, executor faders | `Go`, `Back`, `Pause`, `Release`, fader drag, flash |
| Pools | palettes, groups, worlds, filters, effects | apply, activate, delete |
| Timecode | tracks, events, markers, playhead | start/stop/locate, add event |
| Status | universes, rates, errors, journal, blind/blackout/lock | blackout, lock |

- Nine faders is the first cut (executor numbers 1–9 plus grand), drawn as
  vertical sliders with a keyboard step of 5% and a mouse drag; a fader is the
  only widget that writes a value continuously, and it is throttled to 30 Hz
  with the latest value winning.
- Colour: the sheet shows a colour swatch per fixture when the mode has
  `ColorAdd_*`, computed from the current levels (not from a palette), so the
  operator sees the rig, not the palette. Item 19 extends the same swatch to
  CMY and colour-wheel modes; the window needed no change for it.
- The command line is always present and always the source of truth: every
  action a widget performs is echoed there, so a mouse user learns the commands
  and a keyboard user is never blocked by a missing widget.

### What the GUI must not do

- No engine logic, no render maths, no timecode, no cue tracking.
- No direct writes to `Engine` fields: `pub` access on `Engine.show` is how a
  GUI starts mutating state. The extraction moves every front end onto
  `App::execute` + `App::snapshot`, and `Engine::show` becomes `pub(crate)` with
  accessors, which is a small, mechanical, compiler-checked change.
- No second listing formatter; if a pane needs a column that does not exist, the
  listing gains it and the CLI gains it too.

### Threading

```
UI thread         60 fps draw, snapshot at most every 16 ms
engine thread     Runtime's tick, unchanged
output threads    item 05's workers, unchanged
```

The UI requests a snapshot only when `revision` changed or 250 ms passed (for
status counters), so an idle desk costs one lock every quarter second.

## Implementation plan

1. Extract `crates/app`: `Options` → `App`, move runtime/desk/journal wiring,
   make `Engine::show` non-public with accessors, keep `console` behaviour
   identical (its tests are the regression suite).
2. Add `Snapshot`/`Listings`/`DeskView` with a test that the snapshot is
   consistent at a fixed instant and that building it does not mutate the
   engine.
3. Run the toolkit spike; record the decision and numbers in
   `docs/DESKTOP.md`.
4. Build the shell: window, pane switching, command line, status strip.
5. Add panes one at a time, each with a headless test of its *model* (row
   selection, fader mapping) and no pixel tests.
6. Package as a second binary; keep the terminal shell as the default for
   headless boxes (`openlight --ui` stays).

## Tests

- **app** — the console's existing integration behaviour passes through `App`;
  `App::execute` and the REPL produce identical outcomes for the demo scripts.
- **snapshot** — building a snapshot leaves `revision`, `show` and `desk`
  unchanged; two snapshots at the same revision are equal; a running fade makes
  consecutive snapshots differ only in levels.
- **desktop model** (headless) — a fader drag maps 0..1 to `0..100%` command
  text with a 5% keyboard step; a click on a cue row produces `Go Sequence 1`;
  pane switching does not execute anything; the command line echoes every
  widget action.
- **no-drift** — a test asserts the desktop crate does not depend on
  `engine::render`, `playback` or `timecode` directly (only `openlight-app`),
  which is the structural guard against a second implementation.
- **startup** — the binary starts, builds a snapshot and exits under a
  `--headless-smoke` flag on a machine with no display (CI), proving the model
  half works without a window.

## Boundaries

- No 3D visualiser, no fixture-position rendering (MVR dropped positions in item
  13; there is nothing to draw), no pixel mapping.
- No drag-and-drop cue reordering in the first cut (item 10's `Move` is the
  operation; a list widget can come later).
- No wheel/gobo image editors, no touch/tablet layout, no themes.
- No second binary for macOS/Windows-specific shells; one binary, three
  platforms.

## Risks

- **The GUI becomes a second engine.** The crate boundary, the
  `Engine::show` privacy change and the dependency test are the three guards,
  and they are all mechanical, not disciplinary.
- **Framework lock-in.** Mitigated by the spike and by keeping all UI state in
  `Snapshot`/`DeskView` — swapping toolkits rewrites widgets, not logic.
- **Desktop is the lowest-value item on this list.** It is last for a reason:
  the terminal shell already covers operating the desk, and every item above it
  makes the desk better rather than prettier. If this item slips a release,
  nothing else does.

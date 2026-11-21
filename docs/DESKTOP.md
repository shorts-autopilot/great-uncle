# Desktop GUI

**Status** built in item 18. The terminal shell is still the default on a
headless box; `openlight-desktop` is the same desk with panes, a command line,
draggable faders and a blackout button.

## What the window is

A 1280×800 window with five panes — Sheet, Playback, Pools, Timecode, Status —
selected by F1–F5. The command line is always visible at the bottom and has
focus at start. Nine executor faders and a grand master sit on the right. Every
widget action is echoed above the prompt as command text, so a mouse user learns
the grammar and a keyboard user is never blocked by a missing control.

```bash
cargo run -p openlight-desktop --features gui
cargo run -p openlight-desktop -- --headless-smoke   # no display needed
```

The window is behind the non-default `gui` feature. `cargo test --workspace`
and the MSRV job build and test the model without eframe, a display or a GL
driver; release packaging enables the feature.

| Pane | Shows | Actions |
|---|---|---|
| Sheet | fixture, type, mode, patch, per-attribute level and source (`P`/`Pb`/`H`), colour swatch | `Sel`, `At` the selection to the slider level, `Home`; `Def` sets an attribute/value from the fields, or lists the effective home when they are empty |
| Playback | cues with trigger, fade, delay, MIB and running state | `Sel` chooses a row's sequence without running it, `Go` runs it; the transport bar offers `Go` / `Back` / `Pause` / `Release` |
| Pools | palettes, groups, worlds, filters, effects and active/running state | `Use` / `Off` and `Del` |
| Timecode | shows, tracks, events and markers | `Start` / `Pause` / `Stop`, `Locate` from the time field, and `Add` with the event field |
| Status | the same lines `Status` prints | `BLACKOUT`, `LOCK` and `UNLOCK` in the top bar |
| Faders | nine executor faders, `F` (flash) under each, grand master | drag or use the arrow keys in 5% steps; flash on/off; grand drag |

Every control above writes the command the terminal would receive. The event
field accepts the command's own action text (`Sequence 1 Cue 1`, `Effect 1 On`),
so the grammar stays visible and the engine validates it.

## Crate split

```
crates/app/        load, recover, import, bind transports, start the runtime
                   App::execute(line) and App::snapshot()
crates/console/    the argument parser, scripts, REPL and terminal shell
crates/desktop/    the eframe window and the headless widget model
```

`openlight-app` owns the startup sequence that used to live in
`crates/console/src/main.rs`: show and desk loading, journal replay, autosave,
GDTF/MVR imports, transport binding and the runtime. The console keeps its
argument parser — that is its own product surface — and both front ends call
the same `App`.

The desktop crate depends only on `openlight-app` at the project level. A test
(`crates/desktop/tests/no_drift.rs`) reads its manifest and sources and fails if
`openlight-engine`, `openlight-dmx`, `openlight-remote` or the engine's render,
playback and timecode modules appear. That is a compiler-enforced boundary, not
a convention.

## Snapshot, not shared mutation

```rust
pub struct Snapshot {
    pub revision: u64,
    pub show_name: String,
    pub listings: Listings,   // sheet, playback, pools, timecode and status rows
    pub frame: OutputFrame,
    pub desk: DeskView,
    pub playing: Vec<PlaybackView>,
}
```

The UI thread takes the engine mutex, builds a `Snapshot` (all owned data) and
releases it. Drawing never holds the lock, so a slow repaint cannot stall a
fade and the output thread cannot be blocked by the window. The change poll
reads an atomic shadow of `Engine::revision`, so an idle window does not even
take the engine lock until the 250 ms snapshot is due.

`Listings` is built by the engine, from the same values the `Sheet`, `Status`,
`Sequence`, `Group` and `Timecode` commands print. The desktop parses and
formats no show data of its own. `Engine::show` is now `pub(crate)` with a
`show()` accessor; a front end cannot mutate the show except through
`execute`.

Input goes the other way: a click or key produces a command **string**. A fader
drag produces `Executor 1 At 63`, not a field write. The model in
`crates/desktop/src/model.rs` is headless and tested:

- a 0..1 fader position maps to a whole 0..100 % command;
- a keyboard step is 5 % and clamps at both ends;
- a cue-row click produces `Go Sequence 1`;
- pane switching executes nothing;
- every widget action is echoed, including a command that fails;
- the window refreshes the snapshot when the revision changes, after an action,
  or every 250 ms for status counters;
- fader drags are throttled to 30 Hz with the latest value winning.

## Toolkit spike

The same Playback-pane program — eight cue rows with a Go button and one
vertical fader — was built against each candidate with release settings
(`lto = "thin"`, `strip = true`) on the development host (Linux x86_64,
Rust 1.100.0). Builds were cold, one at a time, and startup is the median of 30
runs of a `--headless` path that parses the arguments and exits before creating
a window.

| | eframe 0.32.3 | iced 0.14.0 | slint 1.19.0 |
|---|---|---|---|
| licence | MIT OR Apache-2.0 | MIT | GPL-3.0-only OR royalty-free 2.0 OR software 3.0 |
| declared MSRV | 1.85 | 1.88 | 1.95 |
| renderer | glow (OpenGL) | tiny-skia software, wgpu optional | femtovg (C++) or software |
| async runtime required | no | no (`thread-pool`) | no |
| release build (cold) | 64 s | 65 s | not built |
| stripped binary | 6.53 MB | 5.59 MB | not built |
| model startup, median | 1.51 ms | 2.58 ms | not measured |
| dependency-tree lines | 317 | 247 | n/a |

The real `openlight-desktop` binary — app, engine, dmx and the window, release
and stripped — is **11.39 MB**, inside the ~20 MB budget; the spike numbers
above are the same Playback pane in isolation, so they compare toolkits rather
than the product.

**Decision: eframe 0.32.3.** All three table columns are acceptance criteria,
not a score. eframe's immediate-mode `Grid` and `Slider` are a direct fit for
five table panes and a row of faders, and its model start is a millisecond
faster. Iced passes the licence, MSRV, pure-Rust and software-renderer tests and
is 0.94 MB smaller; that is inside the ~20 MB budget and did not outweigh the
extra retained-state code every pane would need. Glow is the GL fallback the
design allows; there is no software renderer in this build, so a machine with
no usable OpenGL cannot open the window. Slint is excluded before the spike:
its licence is not MIT/Apache, its MSRV (1.95) is above the workspace's tested
1.89, and its default renderer compiles C++.

**What was not measured.** The build host has no display, so the startup column
is model startup, not window creation. The window is built with both the X11 and
Wayland features and the no-display path is tested, but first-frame latency,
driver compatibility and visual layout on a real desktop were not observed
here. The next person with a desktop should check those and record the result.

## Deliberate limits

No 3D visualiser, fixture-position rendering, pixel mapping, drag-and-drop cue
reordering, wheel/gobo image editors, touch layout or themes. One binary covers
Linux, macOS and Windows; there is no platform-specific shell. The terminal
shell remains the scriptable, headless front end and the GUI adds no command
the console does not accept.

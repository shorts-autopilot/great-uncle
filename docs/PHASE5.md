# Phase 5 — a desk that can be left running

Phase 4 closed the first loop: cues, timecode, move-in-black and undo. Phase 5
is the work that makes the desk safe and expressive in a real room: the show and
the console get separate owners, output has its own clock and telemetry, every
edit survives a crash, the rig can be blacked out and locked, cue lists run
themselves, fades can be split per attribute, and a fixture has a base look
instead of DMX zero.

Each item landed with its own tests, in the order the designs ask for.

## Show data and desk state (03)

`look.olshow` stores programming; `look.oldesk` stores the handles: selection,
programmer, masters, executor fader positions and playback positions. `Save`,
`Save Show`, `Save Desk` and `Load Show` address them independently. A v4 file's
programmer and master positions are handed to the desk on load. Show v5.

## Output scheduling, telemetry and the watchdog (05)

Each universe has a self-clocked worker, so a slow universe cannot stall
another. `Status` reports measured `achieved_hz`, frames, errors, reconnects,
send time and staleness. A broken transport reconnects with backoff; a stalled
engine fades that universe to black rather than freezing the last look.

## Journal, autosave and replay (04)

Every edit is appended to `<show>.journal` as a statement and replayed through
the ordinary command path. `<show>.autosave` is a full snapshot written on a
timer and after every save. `--autosave`, `--journal` and `--no-recover` control
them.

Replay starts after the newest thing the document already contains:

- a successful `Save` writes a checkpoint naming the canonical show path;
- a snapshot records the journal revision it was taken at, including when the
  journal was switched off (the console reads the file's head at startup);
- a snapshot written by an older build without a revision is treated as the
  journal's new head, so its stale tail is never replayed onto it.

A journal that lost its head to rotation or damage starts where the file starts
and says so; a break in the revision chain stops replay.

## Blackout and lock (06)

`Blackout [Fade s]` and `Blackout Off` scale intensity without touching colour,
position or the programming; lifting the blackout is always allowed. `Lock
[code]` / `Unlock [code]` refuse mutating commands through the `mutates()`
allow-list.

## Cue triggers and executor state (01)

A cue stores `Go`, `Follow t` or `Time t`; `Cue n Trigger …` retimes it without
restoring values. `Follow` counts from the settled fade and `Time` from the cue
start, with no wrap past the last cue. Executors gain `Mode Go | Fader | Flash |
Solo`, and their fader, flash and solo handles live in the desk file. Show v6.

## Per-attribute timing, curves and stagger (02)

A cue can override its fade and delay per attribute, give each lane a `Linear`,
`EaseIn`, `EaseOut`, `EaseInOut` or `Snap` curve, and stagger consecutive
fixtures.

```
Store Cue 2 Fade 3
Cue 2 Timing ColorAdd_R Fade 0.5
Cue 2 Timing Dimmer Delay 2.5 Curve Snap
Cue 2 Stagger 0.2
Sequence 1
```

```
Cue 2  Fade 3.0s
  Timing  ColorAdd_R       fade 0.5s  delay 0.0s  Linear
  Timing  Dimmer           fade 3.0s  delay 2.5s  Snap
  Stagger 0.20s between fixtures
```

Internally a run carries one lane per changing attribute; a cue whose lanes all
match its own fade and delay at `Linear` keeps the older single-blend
arithmetic. Pause, resume and a hand on an executor fader rebase the lanes so a
held fade continues where it stopped. A cue settles when its slowest lane does,
so a quick colour fade cannot fire a `Follow` while a long move is still
running. Show v7.

## Fixture defaults and `Home` (07)

The renderer's base look is now `programmer > playback > home > 0`. Home is a
per-fixture map with the profile's `Channel.Default` as its fallback, so an
unedited rig comes up the way the GDTF profile describes instead of at DMX zero.

```
Fixture 1 Default                     list the effective home
Fixture 1 Default Dimmer 100          store an override
Fixture 1 Default Dimmer Off          drop it, back to the profile
Group 1 Default ColorAdd_R 50         the same for every member
Fixture 1 Home / Home / Group 1 Home  recall home into the programmer
```

`Sheet` prints the source next to each value: `P` programmer, `Pb` playback, `H`
home, `-` nothing. Cues still capture the programmer only, so a cue that does
not mention colour does not freeze it. Loading a pre-v8 show reports every
fixture whose profile default is not zero before the operator sees it move.
Show v8.

## Cue-editing safety (10)

A cue can stop tracking: `Cue n Block` materialises the state at that cue and
stores it whole, so later edits to earlier cues no longer move it; `Unblock`
turns it back into a delta. `Cue n Shield <attribute>` asserts one attribute at
the level it has at that cue and stores that level, so the shield survives the
very upstream edit it exists to block; `Cue n Shield` lists what is shielded and
`Cue n Shield <attribute> Off` removes the assertion. Both are visible in the
`Sequence` listing and in `Cue n Show`, which prints the cue's absolute state —
the look it actually produces, not the stored delta.

`Update [Cue n] [Sequence n]` merges the programmer's touched attributes into a
cue, defaulting to the cue that sequence is running, without retyping the list.
`Store Cue n` now merges by default, keeping values the cue already held;
`Store Cue n Replace` keeps exactly what the programmer has. `Cue n Move m` and
`Cue n Copy m [Sequence s]` renumber or copy a cue, timing and shields included,
and `Sequence::rebase` rewrites every delta so no cue's look moves. `Delete Cue`
refuses when a timecode event starts the cue and names the event and position
instead of failing with a generic validation error. Show v9.

## Blind, preview and highlight (09)

`Blind` keeps the programmer off the stage without a second buffer: `At`,
`Store` and `Update` still work and `Sheet` still lists the values, but the
renderer substitutes an empty programmer for as long as blind is on. Playback
keeps running, so editing during a show does not freeze it, and `Blind Off`
makes the edit appear in one frame. Blind is saved with the desk, and `Status`
and the shell header both carry a marker.

`Preview Cue 14 [Sequence 2]` renders that cue's tracked state with no
programmer, effects or masters and prints its first non-zero channels in
`Sheet`; a bare `Preview` inspects the next cue of the running sequence, and a
single-sequence show needs no `Sequence` clause. The frame is only a listing
value, never sent. `Preview Off` clears it, deleting the cue clears it with a
message, and moving the cue carries it to the new number.

`Highlight` holds every selected fixture at full intensity in the renderer,
after the programmer/playback/home chain and before the masters, so a grand
master still dims it. It is never a programmer write, so a store cannot capture
it, and colour and position are untouched. A blackout clears it, blind
suppresses it, and `Clear` (selection gone) and `Release` clear it too.

## 16-bit attributes and channel sets (08)

A profile's fine DMX slot is no longer dropped on import: `Channel.fine_offset`
records the low byte, the mode's footprint covers it, and the renderer writes
both bytes from one interpolated 16-bit value. A 50% pan is `0x7F 0xFF`; an
8-bit profile keeps its existing byte rounding and expands the byte by 257, so
its output does not move. `raw_from_level` is the high byte of
`raw16_from_level`, and `level_from_raw16` is the inverse.

```
Fixture 1 At Pan 50                 one level, two DMX bytes
Fixture 1 Gobo 2                   the second set, by 1-based index
Fixture 1 Gobo "Rotate CW"         by exact, case-insensitive name
Fixture 1 Gobo                     list bands and the current set
Group 1 Gobo "Open"                every member that has the attribute
```

A channel set is the profile's existing `ChannelFunction`, so no new show data
was needed for it. Choosing one stores the midpoint of its physical band as an
ordinary level, which fades and stores like any other value; the listing prints
both the physical band and the DMX bytes, including the fine byte on a 16-bit
channel. A mode that uses one slot twice — coarse or fine — is rejected at load
with both attributes named. Show v10.

## Patch tools and reports (11)

Patching is no longer one fixture at a time. `Clone Fixture 1 At 1.101 Count 8`
copies the source's type, mode, home and name, numbers the copies from the next
free fixture number, and places them at the source mode's footprint apart (or
`Step` slots apart). Every range is checked against the whole patch and the
clones among themselves before anything is inserted, so a failed clone leaves
the rig untouched.

```
Clone Fixture 1 At 1.101 Count 8     eight copies, atomically
Patch Check                          usage, gaps, overlaps, overflow
Patch Import "rig.csv"               replace the patch from a spreadsheet
Report Patch "patch.csv"            one row per fixture, same columns
Report Cues 1 "cues.csv"            the cue list as data
Report Show "show.md"               every shell listing as one document
```

`Patch Check` is read-only and reports gaps larger than eight slots, the largest
one with its address, plus any overlap, overflow, undefined universe or
unresolvable type/mode. `--check` prints it and exits non-zero on a problem, so
a rig can be checked before a fit-up.

The patch CSV has exactly `number,name,type,mode,universe,address`. Import
requires the header, matches it case-insensitively, ignores unknown columns with
a warning, and validates every row before replacing the patch: duplicate
numbers, unknown types, missing modes, overlapping ranges and overflow all name
the row and leave the old patch in place. Quoted fields may contain a comma; a
malformed quote or a row of the wrong width is refused rather than guessed at.

Reports route through the same listing functions the shell uses, so a report
cannot disagree with the screen. They write a file and nothing else: no journal
entry, no undo step, no revision bump.

## Effects: BPM, multi-attribute and phase (14)

An effect can drive several attributes at once, each with its own amplitude and
phase: `Store Effect 1 "Circle" Attributes Pan Tilt Phase Pan 0 Tilt 90` makes
the pair move a quarter cycle apart. `Effect 1 Attributes …` replaces the target
list, and `Effect 1 Attribute Pan Size 30 Phase 120` edits one target without
touching the others. Fixtures missing a target are skipped, and the listing
says how many fixtures lack it.

`Rate 1/4` makes an effect musical: its cycles are derived from the desk tempo
at evaluation time, so `Tempo 128` retimes every running musical effect without
restarting its phase. `Rate 2hz` is a free rate and ignores the tempo;
`Effect 1 Tempo Off` freezes the current derived rate into hertz. `Tempo 128`
sets the desk tempo directly and `Tap` (or `Tap Tempo`) averages the last taps,
with a gap longer than two seconds starting a new count-in. Show v11; a v10
effect becomes one target with its old size and phase 0.

## External timecode and tracks (15)

A timecode show can now follow an external clock. `Store Timecode 1 Source Mtc
"/dev/ttyACM0"` opens a 31 250 8N1 MIDI port when the show starts; `Source Osc`
arms the same follower for positions from the OSC listener, and `Source
Internal` keeps the Phase 4 clock. Quarter-frame and full-frame MTC are decoded,
song-position messages are ignored with a warning, and 29.97 drop-frame is
converted with the two-frames-per-minute rule before the player sees it.

```
Store Timecode 1 "Song" Fps 25 Source Mtc "/dev/ttyACM0"
Timecode 1 Track 2 "FX"
Timecode 1 Track 2 Add 00:00:01:00 Effect 1 On Group 2
Timecode 1 Add 00:00:00:00 Sequence 1 Cue 1
Timecode 1 Marker 00:02:00:00 "Chorus"
```

Tracks are named, numbered and independent: `Timecode 1 Add …` still writes
track 1, old v11 files become one track named `Cues`, and same-frame events fire
in track order. An event is a cue or an effect on/off, and each action goes
through the same engine method the command line uses. Markers are labels and
have no behaviour.

The follower treats a move within ±2 frames as the clock and fires the events
in between; anything larger is a locate and fires nothing. A signal missing for
more than 500 ms marks the player `stalled`, and more than five seconds stops
it. A missing MTC port or a frame-rate mismatch is reported as `source
unavailable` and never silently falls back to the internal clock. Show v12.

## OSC and the browser remote (12)

The console can now be driven from the network. It listens for OSC 1.0 on
`--osc-port` (default 9000) and serves a token-protected page on
`--remote-port` (default 8080), both on loopback unless `--remote-bind` says
otherwise. `--no-remote` turns both off; `--remote-token-file` moves the token
file from `~/.openlight/remote.token`. Binding either port is a startup error,
not a warning.

```
/openlight/executor/1/go           -> Go Executor 1
/openlight/executor/1/fader 0.75   -> Executor 1 At 75
/openlight/blackout 1              -> Blackout
/openlight/fixture/5/at 0.5        -> Fixture 5 At 50
/openlight/timecode 1.5            -> feed the active Osc timecode source
```

OSC is inbound only and is implemented from scratch: padded strings, bundles,
`i`/`f`/`s`/`b`, bounded packets, ignored future timetags. Unknown addresses are
counted and ignored. The page shows executor buttons, the grand master and
blackout, a command box and the engine's replies. `GET /state` is the same state
`Status` reads, `POST /command` runs an ordinary command line, and `GET /events`
pushes state over Server-Sent Events. Every request except the static page needs
the bearer token; a foreign `Origin`, oversized body, missing content type and
an over-limit burst are refused. The remote holds no lighting state: all writes
go through one bounded dispatch queue into `Engine::execute`, so a packet flood
cannot starve the engine tick.

`Status` reports the bound addresses and OSC/HTTP counters. The remote adds no
show or desk fields, so there is no format version change.

## MVR and the fixture builder (13)

`Import Mvr "rig.mvr"` reads the MVR scene description, extracts its embedded
GDTF archives and applies the whole rig as one validated mutation. Fixtures keep
their MVR names and addresses, are numbered from the next free number in
document order, and a name collision gets a numeric suffix. Re-importing a rig
with the same names updates those fixtures instead of duplicating them. Missing
profiles, bad addresses and overlapping ranges skip only the affected fixture
and become warnings; one `Matrix` warning counts the positions the show has no
3D model for. `--mvr <file>` runs the same import before any `--script` line.

Profiles are editable JSON rather than XML. `Import Type` / `Export Type` move
`.olft` files, `Copy Type` duplicates a key, and `Type New`, `Type <key> Mode`
and `Type <key> Channel` build a profile through the same validator. The profile
home map is the fallback after a fixture's stored home and before the channel
default. The show format stays at v12; the new profile home field is optional,
so older documents load unchanged.

## CLI ergonomics, macros and undo groups (16)

The console is usable from scripts and CI. `--version` prints the package and
show format; `--command` runs repeatable statements before a file or stdin
script; `--check [file]` prints only `path: problem` lines and exits 0/1 without
starting output; `--status --json` emits a stable status object and exits;
`--quiet`, `--log <file>`, `--desk <file>` and `--completions bash|zsh|fish`
round out the product surface. `--log` is append-only and separate from the
journal.

`Begin [label]` / `End` group edits into one undo step. A group is visible in
`Status` (`undo group open: N edits`) and in the shell prompt, cannot nest, and
refuses `Undo`/`Redo` until it closes; `Load`, `New` and recovery clear it.
`Patch Import`, `Import Mvr`, `Import Type` and `Macro Run` open a group
internally, so the common bulk operations are atomic without typing `Begin`.

Macros are named statement text stored in the desk file. `Macro <n> "name" …`
stores or replaces, `Add`, `Delete`, `Macro [n]` and `Run` edit or execute it.
Expansion goes through `Engine::execute`, so aliases cannot bypass validation;
the stack refuses a self-call and caps nesting at eight. `Wait` is accepted in
the text but refused at run time, because the engine owns the output tick.

## Release readiness (17)

This item adds no feature. It is the work that makes the previous sixteen safe
to ship.

**Untrusted input.** Every `.gdtf`, `.mvr`, show file and desk file is checked
before parsing, and every limit is named in the error:

| Limit | Value |
|---|---|
| archive entries | 64 |
| one entry, uncompressed | 4 MiB |
| one archive, uncompressed | 16 MiB |
| path shape | relative only: no absolute, drive-qualified or `..` entry |
| channels in one mode | 512 |
| functions on one channel | 4096 |
| fixture types in one show | 1024 |
| show file on disk | 64 MiB, refused from metadata before a read |

The checks live in `crates/engine/src/limits.rs`. Property tests run on every
`cargo test` for the command line, show JSON, desk JSON and timecode parser;
`fuzz/` carries coverage-guided targets for show files, GDTF profiles, MVR
rigs and command input, with the example files as seeds. Fuzzing runs nightly in CI and is not a pull-request
gate.

**Hardware probe.** `openlight --probe` and bare `Probe` send three whole
frames to every detected port (and every configured `--output`), measure the
sustained rate and jitter, and report the break/MAB the transport asks for. A
port that opens but does not finish inside two seconds is reported as
`unknown (no DMX framing observed)` instead of hanging the console. A pty test
covers the real open/configure/write path, a blocking fake covers the timeout,
and ports the runtime already owns are skipped. No real adapter was available,
so the report has not yet been validated on hardware.

**Platforms and MSRV.** Serial framing is `termios2` on Linux and `termios` +
`IOSSIOSPEED` on macOS; any other target builds with a fallback that refuses
`generic:` at open time. CI runs the tests on Linux, macOS and Windows against
stable and the declared MSRV. The MSRV was raised from 1.82 to **1.89** in this
item because the locked dependency set (`time 0.3.55`, `zip 9.8` and
`proptest` for the tests) already required it; the old declaration was not true
and the matrix now tests the real number. `cargo deny` then found two
advisories in `zip 2.6`, so the reader moved to the fixed `zip 9.8` line — the
archive limits themselves are unchanged.

**CI and supply chain.** `.github/workflows/ci.yml` runs fmt, clippy with
`-D warnings`, the workspace tests, the specification check, the demo trace
diff, the platform/MSRV matrix, a USB job and `cargo deny` (`deny.toml`). The
demo traces in `examples/expected/` are the actual last frame of each phase
demo, so a one-byte rendering regression fails a build.

**Packaging.** `packaging/make-tarball.sh` builds the binary and a tarball with
`README.md`, the changelog, security and contributing notes, `docs/`,
`examples/` and `packaging/udev/60-openlight.rules` (group `dialout`, mode
0660). `--version` includes the short git hash when built in a checkout; a
release tarball without a `.git` directory prints the package and format only.

**Benchmarks.** `crates/engine/benches/render.rs` measures one render call on a
release build. These numbers were taken on the development host (Rust 1.100.0,
Linux x86_64) as a hand-check point, not as a CI threshold:

| Bench | Time |
|---|---|
| programmer, 2048 fixtures over 4 universes | 754 µs |
| playback, one cue with 1000 lanes | 840 µs |
| effects, 512 fixtures × 3 targets | 951 µs |

The design asked for 4096 fixtures over four universes. A 512-slot universe
holds at most 512 one-channel fixtures, so four universes physically hold 2048;
the bench uses that maximum rather than a rig the patch validator would reject.

## Desktop GUI (18)

`crates/app` now owns the startup sequence every front end needs: show and desk
loading, journal replay, autosave, GDTF/MVR imports, transport binding and the
runtime. The console keeps its argument parser and calls the shared `App`;
`crates/desktop` draws the same five panes, command line, nine executor faders
and grand master in an eframe window behind the non-default `gui` feature.

Drawing reads an owned `Snapshot` — revision, show name, structured listings,
rendered frame, desk view and playing cues — built under the engine lock and
used after it is released; the revision poll itself is lock-free. A click or drag turns into command text and goes
through `App::execute`; there is no second mutation path. `Engine::show` became
`pub(crate)` with a read accessor, and a test forbids the desktop crate from
depending on the engine, renderer, playback or timecode modules at all.

The toolkit spike, its measurements and the choice of eframe over iced and
slint are in [`DESKTOP.md`](DESKTOP.md). `--headless-smoke` loads, builds a
snapshot and exits without a display, which is how CI tests the model half.
The window itself was compile-checked and the GUI feature passes clippy, but it
has not been opened on a desktop in this environment; first-frame latency and
visual layout remain for the first person with a display.

## The colour model (19)

`Color Red`, `Color "#FF8000"`, `Color Rgb 255 128 0`, `Color Cmy 0 60 100`,
`Color Hsb 30 100 50` and `Color 3200K` write the selection's colour in the
fixture's own system: additive RGB (with white extraction on RGBW), subtractive
CMY, or the nearest named set on a colour wheel. `Fixture 1 Color …` and
`Group 1 Color …` are the same command through a target, and bare `Color` lists
each selected fixture's system, components and hex. `Sheet`'s swatch and the
desktop window follow the same systems, so a CMY fixture is no longer shown as
black. No show or desk field is added; the written values are ordinary
programmer levels that cues, palettes, undo and the journal already understand.

The model is one sRGB space with a fixed named table. It is not a gel library
and not a colour-managed pipeline: a wheel set is matched by its name, and a
set with no colour word is still addressed with `Fixture 1 ColorWheel1 <set>`.
RGBW extraction uses `W = min(r, g, b)` without power normalisation, so white
is one emitter at full rather than four.

## Known limitations

- **No DMX hardware in the test environment.** Timing is exercised against a
  pty and a virtual transport; electrical timing and real serial adapters remain
  unproven.
- **Recovery is path-based.** A snapshot's revision belongs to the journal it
  was taken with; moving a show and its journal to another directory is not a
  supported recovery workflow.
- **Home is per fixture and attribute.** There is no per-mode home table, no
  park/default distinction and no automatic re-seed when a profile is
  re-imported.
- **The desktop window has not been opened on a real display here.** The model
  half is tested headless, the GUI feature compiles and passes clippy, and the
  X11/Wayland builds are wired, but window creation, first-frame latency and
  visual layout were not observed in this environment. The build host has no
  display, so the glow renderer has not rendered a frame. See
  [`DESKTOP.md`](DESKTOP.md).
- **The hardware probe has not met a real adapter.** Its timing, timeout and
  report logic are tested against a pty and a blocking fake, and the macOS
  backend is compile-checked, but electrical timing on actual USB-serial
  hardware remains unmeasured. `--probe` is the tool for the first person who
  can run it; report the numbers rather than assuming them.
- **Windows has no serial backend in this build.** `virtual` works, while
  `generic:` and `enttec:` both report `unsupported`: ENTTEC shares the serial
  layer rather than owning a separate COM path. The workspace still builds and
  runs its tests on Windows.
- **Fuzzing is a nightly job, not a pull-request gate.** The property tests run
  on every test invocation; the fuzz corpus only grows when the nightly job
  finds a regression.
- **Benchmark numbers are host-specific.** They are a regression check for a
  developer, not an assertion in CI.
- **MTC was tested with synthetic bytes, not hardware.** Quarter-frame and
  full-frame decoding, drop-frame maths and the reader thread are covered by
  unit tests; a real MIDI interface remains unverified.
- **Macros are linear statement lists.** No variables, conditionals, loops or
  key/button triggers; a macro runs through the ordinary command path and stops
  at the first error.
- **MVR is a patch/profile carrier.** Geometry, models, trusses and layers as
groups are ignored, and there is no MVR export. Real archives vary in
dialect; the parser accepts namespaces and a missing `<Scene>` wrapper, but it
has not been run against a large vendor library.
- **The remote is plain HTTP and loopback by default.** There is no TLS; a
  token on a trusted show network is the security model, and `--remote-bind
  0.0.0.0` is an explicit opt-in. OSC has no authentication at all, so expose
  its port only on a trusted network.

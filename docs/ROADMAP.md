# Roadmap — designs for the work after Phase 4

Phase 4 closed the first loop: the desk plays cues, follows timecode, moves in
black and can undo. This document is the **design set for the next eighteen
pieces of work**, ranked by value to a working desk. Each item has a design in
[`docs/design/`](design/). All eighteen are built, in the order the designs ask
for:

| Item | State |
|---|---|
| 03 show data vs desk state | **built** — `.oldesk` beside the show, `Save Show` / `Save Desk` / `Load Show`, undo covers the programmer, v4 files seed the desk |
| 04 autosave, journal and replay | **built** — `<show>.journal` replayed at start, `<show>.autosave` snapshot, `--autosave`, `--journal`, `--no-recover`, `Status` names both; replay starts after the newest save or snapshot, and a snapshot taken with the journal off is never replayed onto |
| 05 output scheduling, telemetry, reconnect, watchdog | **built** — one self-clocked worker per universe, measured `achieved_hz` / `last_send_us`, reconnect with backoff, stale-then-black watchdog |
| 06 reversible blackout and desk lock | **built** — `Blackout [Fade s]`, `Blackout Off`, `Lock [code]`, `Unlock [code]`, `Command::mutates()` allow-list |
| 01 cue triggers and executor state | **built** — `Store Cue n Follow t` / `Time t`, `Cue n Trigger …`, `Executor n Mode`, `Flash`, `Solo`; show v6; executor handles in the desk file |
| 02 per-attribute timing, curves and stagger | **built** — cue `timing` map and `stagger`, `Cue n Timing …`, `Cue n Stagger`, one fade lane per attribute with quadratic curves, show v7 |
| 07 fixture defaults and `Home` | **built** — per-fixture home with the profile's `Channel.Default` fallback, `Fixture n Default …`, `Home`, sheet sources `P`/`Pb`/`H`, show v8 |
| 10 cue-editing safety | **built** — cue `blocked`/`shield`, `Sequence::rebase`, `Update`, `Store … Merge`/`Replace`, `Cue n Move`/`Copy`/`Show`, `Delete Cue` names a blocking timecode event, show v9 |
| 09 blind, preview and highlight | **built** — desk `blind`/`highlight` and a transient preview, `Blind [Off]`, `Preview [Cue n] [Sequence n]` / `Preview Off`, `Highlight [Off]`, highlight as a render-stage override that no store can capture |
| 08 16-bit attributes and channel sets | **built** — `Channel.fine_offset`, one 16-bit interpolation split into coarse and fine bytes, `raw16_from_level` / `level_from_raw16`, named sets addressable by index or exact name, slot-collision validation, show v10 |
| 11 patch tools and reports | **built** — atomic `Clone Fixture … Count … Step …`, read-only `Patch Check` with gap/overlap/overflow reporting, `Patch Import` / `Report Patch` through a minimal CSV, `Report Cues|Palettes|Status|Show`, `--check` |
| 14 effects, BPM and multi-attribute waveforms | **built** — `EffectTarget` list with per-target size and phase, `BeatDivision` musical rates evaluated against desk tempo, `Tempo` / `Tap Tempo`, target/rate editing commands, show v11 |
| 15 external timecode and tracks | **built** — timecode `source` (`Internal`, `Mtc "port"`, `Osc`), named tracks with cue/effect actions, markers, MTC quarter/full-frame decoding with drop-frame maths, ±2-frame following with locate/stall/stop, show v12 |
| 12 OSC and web remote | **built** — inbound OSC 1.0 codec with bundles, ten address mappings plus OSC timecode, token-protected HTTP page/state/command/SSE, bounded dispatch queue, `Status` counters; no format change |
| 13 MVR and fixture builder | **built** — MVR zip scene walk, embedded GDTF extraction, per-fixture warnings, next-free numbering and re-import updates; `.olft` import/export/validation, `Type New`/`Mode`/`Channel`, `Copy Type`, `--mvr` |
| 16 CLI ergonomics, macros, undo grouping | **built** — `--version`, repeatable `--command`, `--script -`, stable `--check`, `--status --json`, `--quiet`, `--log`, `--desk`, completions; desk macros with cycle/depth guards, `Begin`/`End` groups and grouped bulk imports |
| 17 release readiness | **built** — archive/document limits, property tests and fuzz targets, `--probe`, macOS serial backend and a portable fallback, CI matrix with a tested MSRV, licences and advisories, tarball and udev rules, render benchmarks |
| 18 desktop GUI | **built** — `openlight-app` wiring, owned snapshots and structured listings, an eframe window behind the `gui` feature with the command line and a headless `--headless-smoke` path; item 25 replaces its fixed panes with the window workspace |

The designs are written to be implemented one at a time. Every one names the
files it touches, the command grammar it adds, the show/desk format change it
makes (if any), the tests that prove it, and what it deliberately leaves out.
The full-manual audit — every row classified, every out-of-scope row with a
reason — is in [`COMPLETENESS.md`](COMPLETENESS.md); the eighteen designs are
the build plan for the desk, not the whole manual.

## The list

Rank is value to an operator in front of a rig: how much a broken workflow is
fixed, times how many shows it touches, divided by the risk of getting it wrong.
Order is the order the designs should be built in, which is not the same thing.

| Rank | Item | Design | Size | Order | Show | Desk |
|---|---|---|---|---|---|---|
| 1 | Cue triggers and executor state | [01](design/01-cue-triggers-and-executors.md) | L | 5 | v6 | d1\* |
| 2 | Per-attribute timing, curves and stagger | [02](design/02-attribute-timing-curves-stagger.md) | L | 6 | v7 | — |
| 3 | Show data vs desk state | [03](design/03-show-and-desk-state.md) | L | 1 | v5 | d1 |
| 4 | Autosave, journal and replay | [04](design/04-autosave-journal-replay.md) | M | 3 | — | — |
| 5 | Output scheduling, telemetry, reconnect, watchdog | [05](design/05-output-scheduling-telemetry.md) | M | 2 | — | — |
| 6 | Reversible blackout and desk lock | [06](design/06-blackout-and-desk-lock.md) | S | 4 | — | d1\* |
| 7 | Fixture defaults and `Home` | [07](design/07-fixture-defaults-and-home.md) | M | 7 | v8 | — |
| 8 | 16-bit attributes and channel sets | [08](design/08-sixteen-bit-and-channel-sets.md) | M | 10 | v10 | — |
| 9 | Blind, preview and highlight | [09](design/09-blind-preview-highlight.md) | M | 9 | — | d1\* |
| 10 | Cue-editing safety | [10](design/10-cue-editing-safety.md) | L | 8 | v9 | — |
| 11 | Patch tools and reports | [11](design/11-patch-tools-and-reports.md) | M | 11 | — | — |
| 12 | OSC and web remote | [12](design/12-osc-and-web-remote.md) | M | 14 | — | — |
| 13 | MVR import and a fixture builder | [13](design/13-mvr-and-fixture-builder.md) | L | 15 | — | — |
| 14 | Effects: BPM, multi-attribute, phase | [14](design/14-effects-bpm-multi-attribute.md) | M | 12 | v11 | d1\* |
| 15 | External timecode and tracks | [15](design/15-external-timecode-and-tracks.md) | L | 13 | v12 | — |
| 16 | CLI ergonomics, macros, undo grouping | [16](design/16-cli-ergonomics-macros.md) | M | 16 | — | d1\* |
| 17 | Release readiness | [17](design/17-release-readiness.md) | L | 17 | — | — |
| 18 | Desktop GUI | [18](design/18-desktop-gui.md) | L | 18 | — | — |

`—` means the item adds no persisted fields; `d1*` means it adds one field to
the desk file, which needs no version bump (see the ladder below). Show versions
are allocated by implementation order, so the numbers above are a schedule, not
a preference: item 3 must land before anything that reads a desk file.

### After the set

A follow-up review ranked the work outside these eighteen. The hardware and
display proof it put first remains the release gate: no row is `verified`
until it has run on real DMX hardware and a real display. The first seven
buildable items from that review are designed and built:

| Item | State |
|---|---|
| [19 the colour model](design/19-colour-model.md) | **built** — `Color` by name, hex, RGB, CMY, HSB or kelvin; additive, CMY and wheel assignment; RGBW extraction; sheet swatches for every system; no format change |
| [20 executor pages](design/20-executor-pages.md) | **built** — named pages, page-qualified executor numbers and runs, `Page`/`Store Page`/`Delete Page`, per-page handles, show v13 with the v12 upgrade, active-page GUI and remote views |
| [21 DMX in and merge](design/21-dmx-in-and-merge.md) | **built, hardware-gated** — virtual and ENTTEC Pro readers, per-universe HTP/LTP merge, local/input priority, hold/zero/fade loss, `Dmx` status and meter, rising-edge `Go Sequence` trigger, programmer snapshot, show v14; physical input remains unproven until the hardware pass |
| [22 MAtricks, Shuffle and Align](design/22-matricks-shuffle-align.md) | **built** — block/group/wing/width selection cells, deterministic seeded shuffle, mirror transform, `Align` across selection order with linear/sinus/slow/fast curves, fine-channel resolution; no show format change |
| [23 Windows serial backend](design/23-windows-serial.md) | **built, hardware-gated** — native COM link with 250000 8N2/57600 8N1 and break/MAB, ENTTEC over COM, registry COM discovery in `--probe`, Windows CI tests and a portable install script; no adapter timing measured |
| [24 File management, backups and templates](design/24-file-management-backups-templates.md) | **built** — automatic ten-deep timestamped backups, `Files`, `Save As`, explicit `Backup`, companion-aware `Copy`/`Rename`/`Delete Show`, and show-only `Save`/`Load Template`; no format change, no active-path rebinding |
| [25 Window workspace](design/25-window-workspace.md) | **built, UI being rebuilt** — the headless workspace model (12 × 8 grid, placement, non-overlap, drag/resize rules, 16-window catalogue) and the command-only model are tested; the GUI chrome, command line and picker contents were deliberately removed; clicking the canvas opens the themed Add Window picker with the catalogue tabs, choosing a tile places that window at the clicked cell, and the UI is being rebuilt piece by piece on the operator's direction. Layout is session-only, no format change; visual/drag confirmation remains part of the display release gate |

## Why this order

The first four steps make the desk safe to leave running before it gets more
features:

1. **03 (state split)** — until the programmer stops living in the show file,
   every autosave, undo and remote command has the wrong owner for half its
   state. Everything else reads this model.
2. **05 (output)** — a stalled engine currently freezes the last look on stage
   and a yanked cable silently stops that universe. This is the only item that
   can damage a show in progress.
3. **04 (journal)** — requires 03 so a journal replays a document that is
   actually the document. Autosave without the split would save the programmer
   into the show, the bug 03 exists to fix.
4. **06 (blackout/lock)** — two small desk flags that need the desk file 03
   introduced.

Then the operator-facing work, in the order an operator would miss it: a cue
list that can run itself (01), fades that behave like a desk (02), a base look
beneath everything (07), cue editing that cannot silently change other cues
(10), a way to see what you are doing without the rig seeing it (09), and
attributes that need more than one byte (08).

Then reach: release engineering (17) is the work that makes the first sixteen
safe to ship, and only after it does a native window (18) make sense.

## Three kinds of state

Every design in this set is careful about which of these it writes:

| Kind | Lives in | Written by | Saved |
|---|---|---|---|
| **Show** | `Show`, `.olshow` | `Store …`, `Patch`, `Delete …` | yes, `Save` |
| **Desk** | `Desk`, `.oldesk` | programming, handles, transport, locks | yes, on `Save` and autosave |
| **Session** | process memory only | the output runtime | never |

- **Show** is what you programmed: fixture types, patch, palettes, cues,
  sequences, groups, worlds, filters, effects, timecode, fixture home values.
- **Desk** is where the handles are and what you are doing now: selection,
  programmer values, active world and filter, master and fader positions,
  running playback positions, effect runs, timecode playhead, blackout, lock,
  macros, tempo, MAtricks shape and the Align transition.
- **Session** is what describes the machine: output status, undo history,
  scroll positions, the journal handle.

The rule that decides a borderline case: *if losing it would only mean moving a
fader back, it is desk state; if losing it would mean reprogramming, it is show
state.*

## One writer, one grammar

Every new surface — the terminal shell, a macro, OSC, the web remote, the GUI —
turns input into command text and calls `Engine::execute`. No design here adds
a second path into the show, and none adds a second renderer: listings and
reports are produced by the same functions the command line prints.

New commands follow the existing shapes:

| Shape | Example | Used by |
|---|---|---|
| `Noun <n> Verb [value]` | `Cue 2 Trigger Follow 1.5` | 01, 02, 10 |
| `Verb [Noun <n>]` | `Blackout`, `Home` | 06, 07 |
| `Noun <n> Verb Off` | `Blind Off`, `Highlight Off` | 06, 09 |
| `Verb Noun <n> …` | `Clone Fixture 1 At 1.101 Count 4` | 11 |
| `Import <kind> <file>` | `Import Mvr "show.mvr"` | 13 |

Bare words are reserved for toggles (`Blackout`, `Blind`, `Highlight`) which
read as the action they start, and every one of them has an explicit `Off`
form so a script never has to guess the current state.

## Format ladder

Show and desk files are versioned separately, and every reader keeps working
for every older version. This is the schedule the designs assume.

| Version | Adds | Item |
|---|---|---|
| show v5 | programmer, masters and executor faders move out of the show | 03 |
| show v6 | cue `trigger` and `trigger_time`, executor `mode` | 01 |
| show v7 | cue `timing` map, `stagger` | 02 |
| show v8 | fixture `home` | 07 |
| show v9 | cue `blocked`, `shield` | 10 |
| show v10 | channel `fine_offset`, named channel sets | 08 |
| show v11 | effect `targets`, `tempo` | 14 |
| show v12 | timecode `source`, `tracks`, markers | 15 |
| show v13 | `pages`, executor `page`; page one is implicit | 20 |
| show v14 | universe input `merge`, `priority`, `loss`, `input_timeout_ms`, `input_trigger` | 21 |
| desk v1 | programmer, selection, world/filter, masters, run positions | 03 |

The desk file is additive: later items add `#[serde(default)]` fields without a
version bump (executors in 01, blackout/lock in 06, highlights in 09, tempo in
14, macros in 16, pages and per-page handles in 20). Its version only moves
when a field is removed or renamed,
because a desk file is disposable and a reader that ignores an unknown field is
safer than a migration for every addition. Item 22's `matricks` and
`align_transition` fields are the latest additions.

A version bump is a **reader** change: the old fields stay legible, the loader
upgrades in memory, and saving writes the new version. `Show::load` grows a
companion `load_with_report` returning a `LoadReport { warnings, desk_seed }`
so an upgrade can hand migrated values (a v4 programmer, a v6 grand-master
position) to the desk without keeping dead fields in the current struct.

## How these designs are verified

Each design lists its own tests. The standard every one of them meets:

- unit tests in the module that owns the behaviour (`playback`, `render`,
  `show`, `command`, `desk`, …), driven by explicit instants, never `sleep`;
- one engine-level test that goes through `Engine::execute`, because that is
  the only path the operator has;
- a rejected-input test for every new command, asserting the message names the
  value that was wrong;
- a save/load round-trip whenever the format changes, plus an upgrade test from
  the previous version;
- `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` clean, and no new `unsafe` outside `crates/dmx`.

Demos (`examples/*.txt`) are added only when an item changes what the standard
demo prints. Most of these designs extend the existing phase demos rather than
adding new ones.

## Not in this set

Deliberately left out, so no design quietly grows into them:

- **Art-Net or sACN output.** The project's constraint is serial DMX out.
- **LTC audio decoding.** Needs an audio capture stack and DSP; item 15 does
  MIDI timecode and OSC instead, and says why.
- **3D visualisation, wheel/position editors, pixel mapping.** Item 25 adds a
  user-defined window workspace over the supported snapshot views, but not the
  deferred viewers, editors or media pools that need new engine data.
- **Multi-user or networked show collaboration.** No authentication model
  exists to build on, and the desk is single-operator by design.
- **Third-party fixture libraries.** GDTF and MVR remain the only sources.

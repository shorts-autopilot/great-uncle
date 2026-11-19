# Roadmap — designs for the work after Phase 4

Phase 4 closed the first loop: the desk plays cues, follows timecode, moves in
black and can undo. This document is the **design set for the next eighteen
pieces of work**, ranked by value to a working desk. Each item has a design in
[`docs/design/`](design/). Three of them are built so far, in the order the
designs ask for:

| Item | State |
|---|---|
| 03 show data vs desk state | **built** — `.oldesk` beside the show, `Save Show` / `Save Desk` / `Load Show`, undo covers the programmer, v4 files seed the desk |
| 05 output scheduling, telemetry, reconnect, watchdog | **built** — one self-clocked worker per universe, measured `achieved_hz` / `last_send_us`, reconnect with backoff, stale-then-black watchdog |
| 06 reversible blackout and desk lock | **built** — `Blackout [Fade s]`, `Blackout Off`, `Lock [code]`, `Unlock [code]`, `Command::mutates()` allow-list |
| the other fifteen | designed, not built |

The designs are written to be implemented one at a time. Every one names the
files it touches, the command grammar it adds, the show/desk format change it
makes (if any), the tests that prove it, and what it deliberately leaves out.

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

The last eight are reach: patch tooling (11), remote surfaces (12, 13),
animation and timecode depth (14, 15), the command line as a product (16),
release engineering (17), and only then a native window (18).

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
  macros, tempo.
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
| desk v1 | programmer, selection, world/filter, masters, run positions | 03 |

The desk file is additive: later items add `#[serde(default)]` fields without a
version bump (executors in 01, blackout/lock in 06, highlights in 09, tempo in
14, macros in 16). Its version only moves when a field is removed or renamed,
because a desk file is disposable and a reader that ignores an unknown field is
safer than a migration for every addition.

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
- **3D visualisation, wheel/position editors, pixel mapping.** Item 18 stops at
  the five listings plus a command line.
- **Multi-user or networked show collaboration.** No authentication model
  exists to build on, and the desk is single-operator by design.
- **Third-party fixture libraries.** GDTF and MVR remain the only sources.

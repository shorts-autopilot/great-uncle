# Phase 4 — timecode, move-in-black, undo and the first UI

Phase 3 made the rig manageable. Phase 4 makes the desk playable and
recoverable: cues can follow a **timecode**, a cue can **move in black**, every
show edit can be **undone and redone**, and the console finally has a **shell**
instead of only a prompt. The engine is still the single writer; the shell is
another surface that speaks the same command line.

## Undo and redo

Every command that changes the show records the show as it was before the
change, with the command text as its label. `Undo` restores that snapshot and
`Redo` re-applies it.

```
Fixture 1 At 50
Store Cue 1 Fade 2
Undo                       undid `Store Cue 1 Fade 2`
Undo                       undid `Fixture 1 At 50`
Redo
```

```text
Undo: 14 steps | Redo: 0 steps
```

- **Snapshots, not inverses.** A snapshot is the whole validated show, so undo
  restores fixtures, palettes, cues, groups, worlds, filters, effects, masters
  and timecode events together. There is no per-feature inverse to get wrong.
- **Only real changes.** A command that fails or leaves the show identical
  records nothing. A new edit after an undo drops the redo branch, like an
  editor.
- **Console state is not undone.** Selection, running playbacks, running
  effects, the timecode playhead and the output status are not show data, so
  `Undo` leaves them alone. If undo removes a sequence or effect that is
  running, that run is dropped rather than left pointing at nothing.
- **Bounded.** The last 64 edits are kept; older snapshots fall off.
- `New` and `Load` start a fresh history, because the previous show is no longer
  the document being edited.

## Move-in-black

A cue can be stored with the **Mib** flag:

```
Store Cue 2 "Red" Fade 1 Mib
Store Cue 3 "Red" Mib Off
Sequence 1
```

```text
Cue    Name                   Fade  Delay Mib  Values
1      Dark                   0.0s   0.0s -    4
2      Red                    1.0s   0.0s yes  4
```

When a MIB cue starts, every fixture that is **dark** in the state the cue
starts from jumps straight to the cue's **non-intensity** values (colour,
position, beam — everything that is not `Dimmer`/`Intensity`). Intensity is
left alone and still fades. A fixture that is already lit is untouched, so MIB
never makes a visible move.

The effect is that a moving light is already in its new position and colour
while its dimmer is at zero, and comes up into that look instead of fading
through it. This is the snapshot-at-cue-start form of move-in-black: there is
no separate MIB fade time or MIB delay yet.

## Timecode

A **timecode show** is a numbered list of events. Each event is a position
(`HH:MM:SS:FF` at the show's frame rate) and the cue it starts.

```
Store Timecode 1 "Song" Fps 25
Timecode 1 Add 00:00:00:00 Sequence 1 Cue 1
Timecode 1 Add 00:01:30:00 Sequence 1 Cue 2
Timecode 1 Add 00:02:10:00 Sequence 1 Cue 3
Timecode 1
```

```text
Timecode 1 "Song" — 25 fps, 3 events
Time           Sequence    Cue
00:00:00:00           1      1
00:01:30:00           1      2
00:02:10:00           1      3
```

Transport:

```
Timecode 1 Start          play from the current position (0 when stopped)
Timecode 1 Pause          freeze the playhead
Timecode 1 Stop           stop and rewind to 00:00:00:00
Timecode 1 At 00:01:00:00 locate without firing the events it passes
```

The playhead runs on the engine clock, in frames, with fractional frames kept
so repeated ticks do not drift. On every tick the engine fires the events the
playhead passed, in order, by starting that cue on its sequence at full
executor level. Events that point at a deleted cue are skipped rather than
stopping the track.

```text
Timecode: timecode 1 running 00:00:01:12 (25 fps)
```

Timecode shows are show data; the player is not. Loading a show starts with
everything stopped, like playback in Phase 1.

## The UI shell

`openlight --ui` opens a full-screen terminal shell:

```text
 OpenLight — Untitled Show                        timecode 1 stopped 00:00:00:00 (25 fps)
 [Sheet]  Playback   Pools   Timecode   Status
 --------------------------------------------------------------------------------
 No.  Name                   Type                       Mode       Patch      Output
 1    Demo Wash              OpenLight Demo Wash        4ch        1.001      Dimmer 100%  Red 100% …
 --------------------------------------------------------------------------------
   > Stored cue 2 "Red" in sequence 1 (fade 1s, delay 0s, mib, 4 tracked values)
 openlight> Fixture 1 At 50
 Tab window  Enter run  Up/Down history  PgUp/PgDn scroll  Esc clear  Ctrl-D quit
```

The five windows are the same listings the command line already prints — the
fixture sheet, playback, the pools, timecode and the status report — so there is
one description of every pool, not a second renderer that can drift.

| Key | Action |
|---|---|
| printable keys | edit the command line |
| Enter | run the line through `Engine::execute` |
| Up / Down | command history |
| Tab | next window |
| PgUp / PgDn | scroll the body |
| Esc | clear the line and the message |
| Ctrl-C | discard the line |
| Ctrl-D, `exit` | leave the shell |

The shell is a **terminal** interface, not the desktop GUI: it needs a tty, it
uses raw mode and the alternate screen, and it restores the terminal on exit and
on panic. Rendering and key handling are pure functions over engine state, so
the layout, the prompt, history, error reporting and clipping are tested without
a terminal.

## Show format v4

Version 4 adds one section:

```json
"timecodes": [
  {
    "number": 1,
    "name": "Song",
    "fps": 25,
    "events": [{ "at": "00:00:00:00", "sequence": 1, "cue": 1.0 }]
  }
]
```

Version 1, 2 and 3 documents still load, upgrade on read and save as version 4.
Validation covers:

- timecode: non-zero unique number, frame rate 1..=60, event limit;
- event: a parseable position inside the rate (`00:00:00:25` is invalid at
  25 fps), no two events at the same frame, ascending order;
- event targets: the sequence must exist and must contain the cue.

`mib` is a JSON boolean on a cue; a document with anything else is rejected by
the deserializer before validation.

## Command summary

| Command | Effect |
|---|---|
| `Undo`, `Redo` | step through show edits |
| `Store Cue 2 "name" [Mib] [Mib Off]` | store a cue with move-in-black |
| `Store Timecode 1 "Song" [Fps 25]`, `Delete Timecode 1`, `Timecode [1]` | timecode shows |
| `Timecode 1 Add 00:00:10:00 Sequence 1 Cue 2` | add an event |
| `Timecode 1 Remove 00:00:10:00` | delete an event |
| `Timecode 1 Start`, `Pause`, `Stop`, `At 00:00:10:00` | transport and locate |

## Evidence

`cargo test --workspace` runs **184 tests**: 6 in `console`, 11 in `dmx` and 167
in `engine` (30 more than Phase 3). The new coverage:

- **history** — undo/redo, the redo branch, the depth cap, clearing.
- **timecode** — position parse/format at different rates, rejected positions,
  events firing once and in order, fractional-frame progress, pause and locate.
- **playback** — MIB snaps colour on a dark fixture and leaves a lit one fading.
- **show** — v3 files upgrade to v4, event validation, event targets, v4 save.
- **command** — timecode grammar, `Mib` and `Mib Off`, `Undo`/`Redo`.
- **engine** — undo and redo across programmer and store edits, no step for a
  rejected command, a new edit dropping redo, MIB through a real fade and cue
  listing, timecode firing cues on the engine clock, pause/resume/stop/locate,
  bad timecode input, timecode save/load, status fields.
- **ui** — header, window bar, padded body, prompt, clipping at 40 columns, a
  line running through the engine, errors in the message line, history recall,
  window cycling and quit keys.

The demo (deterministic, because the track has finished):

```bash
cargo run -q -p openlight-console -- --output 1=virtual \
  --script examples/phase4-demo.txt --frames 120 --trace
```

```text
Universe 1: 120 frames, last frame 513 bytes, start code 0x00, 4 non-zero slots
  00 FF FF 00 00 FF FF 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 …
```

`FF` at address 1 is the dimmer of fixture 1 at full and `FF` at address 2 its
red channel; the second fixture repeats the pair at 5 and 6.

## Known limitations

- **Timecode is internal.** There is no LTC or MIDI timecode input, no external
  connections, no timecode slots and no track/marker/range model: one track of
  cue-trigger events per show.
- **Timecode events start cues on their sequence**, not through an executor, so
  an executor fader does not scale a timecoded cue.
- **MIB has no timing.** It snaps non-intensity values at cue start; there is no
  separate MIB fade, MIB delay or "MIB always/never" setting.
- **Home is per fixture.** An unprogrammed attribute renders the profile's
  `Channel.Default`, but there is no per-mode home table, no park/default
  distinction, and re-importing a profile does not re-seed an edited home.
  Loading a pre-v8 show reports every fixture whose profile default is non-zero
  before it moves.
- **Undo covers the show file only.** It does not undo console state such as
  selection, playback or effect starts, and a load resets it.
- **The UI is a terminal shell.** There is no desktop GUI, no mouse, no colour,
  and no per-window settings; it needs a Unix tty.
- The Phase 1–3 limitations that still apply (no tracking shield, no instant
  release, playback state unsaved, text-only sheets) are unchanged.

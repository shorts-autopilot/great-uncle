# Phase 2 — programming and playback

Phase 2 turns the Phase 1 engine into something you can run a show with:
palettes, cues stored with tracking, sequences, executor faders, and timed
playback with fades and delays. It stays headless and keeps the two rules that
made Phase 1 safe: one writer, and a show that is always valid.

The milestone: **store two cues, run them from an executor with its fader at
75%, and see the output crossfade on the wire.**

## Scope

Done in this phase:

- a separate **programmer** and **playback** layer, merged at render time;
- attribute addressing (`Fixture 1 At ColorAdd_R 40`, pretty names too);
- typed **palettes** per feature pool, stored per fixture type;
- **cues** with tracked deltas, fractional numbers, fade and delay;
- **sequences** with Go, Back, Pause, Release, and a deterministic clock;
- **executors**: assign a sequence, drive a master fader, Go/Back/Pause/Release;
- show format **version 2**, still reading version 1;
- `Wait <seconds>` in the console script runner, so a script can let a fade run.

Not in this phase: effects, groups, worlds and filters, cue-only/tracking
shield, recipe presets, timecode, move-in-black, undo, and any GUI.

## The two layers

Phase 1 wrote levels straight into the fixture. Phase 2 separates them:

```
        commands                    cues / sequences / executors
            │                                   │
            ▼                                   ▼
   ┌─────────────────┐                 ┌──────────────────┐
   │ programmer      │                 │ playback         │
   │ fixture.levels  │                 │ runs, fades, HTP │
   └────────┬────────┘                 └────────┬─────────┘
            │                                   │
            └───────────────┬───────────────────┘
                            ▼
                    render(show, playback)
                            │
                            ▼
                        DMX frame
```

Three consequences, worth knowing before programming:

1. **Programmer wins.** A value the operator holds wins over playback for that
   attribute. `Clear Values` removes the programmer values of the selection and
   hands the attributes back to playback; `Clear All` clears the selection and
   the whole programmer.
2. **Patching starts dark.** A fixture has an empty programmer, not a map of
   zeroes, so playback can drive it immediately after patching.
3. **Playback is not saved.** Cue data and fader positions are part of the show;
   "which cue is running" is not. Loading a show starts idle.

Across several running playbacks, intensity merges highest-takes-precedence
(HTP) and every other attribute latest-takes-precedence (LTP), in target order
(sequences before executors, each by number). A single executor is the common
case and is unaffected.

## Tracking

A cue stores only the values that differ from the state the previous cues leave
behind. The state of cue *n* is the accumulation of cues `0..=n`, and a fade
interpolates from the state before the cue to the state of the cue.

```
cue 1  Dimmer 20   stored 20
cue 2  Dimmer 100  stored 100   (Dimmer changed)
cue 3  Red 100     stored Red   (Dimmer is inherited, not re-stored)
```

Editing an earlier cue changes what later cues inherit, exactly like tracking on
a console. Storing an existing cue number updates that cue, so `Store Cue 2`
twice is an edit, not a duplicate. Deleting a cue leaves later cues untouched.

`Sequence 1` shows each cue's stored value count, which is the quickest way to
see whether tracking did what you expected.

## Palettes

A palette is a **typed pool**: Dimmer, Color, Position, Beam, Gobo, Focus,
Shapers, Control, or `Other`. The pool comes from the fixture type's own GDTF
`Feature` (the part before the first dot), falling back to the standard
attribute name prefixes, and is never invented for a profile.

Values are stored **per fixture type**, because a wash and a profile spot do not
share a colour system:

```
Fixture 1 At ColorAdd_R Full; At ColorAdd_B 30
Store Palette Color 1 "Deep Blue"      # stores R=1, G=0, B=0.3 for this type
Fixture 2 At Palette Color 1           # applied to fixture 2's own values
```

- Capturing stores every attribute of the pool that the mode has, including
  attributes at 0: a colour palette stores all of RGB.
- Applying sets those values in the programmer and reports fixtures it had to
  skip because the palette holds nothing for their type.
- Palette numbers are unique inside a pool, so `Color 1` and `Beam 1` coexist.
- Re-storing a number replaces its values; `Store Palette` never merges.

Palette pools depend on the profile. A profile without feature metadata still
gets sensible pools from attribute names (`ColorAdd_*` → Color, `Pan`/`Tilt` →
Position, `Zoom`/`Focus`/`Gobo` → Beam).

## Playback model

| Command | Effect |
|---|---|
| `Go [Executor 1 \| Sequence 1]` | next cue, or resume a paused one |
| `Back [...]` | previous cue, using that cue's fade |
| `Pause [...]` | freeze the current fade where it is |
| `Release [...]` | stop the playback; its output disappears immediately |
| `Executor 1 At 75` / `Full` / `Off` | master fader |

With no target, `Go`/`Back`/`Pause`/`Release` use executor 1, or the first
sequence when no executor is assigned.

- **Fade and delay** come from the cue being entered. A zero fade is instant.
- **Pause** freezes the interpolated output; `Go` on a paused playback resumes
  the remaining fade time instead of advancing.
- **Back** starts the previous cue with that cue's own fade.
- **Release** is a hard stop; there is no release fade yet.
- **The master fader scales intensity only** (the `Dimmer` attribute). Colours
  and positions keep their programmed values, so pulling a fader down dims a
  look instead of desaturating it.
- **Reassigning an executor** stops whatever it was running; the fader keeps
  its position.
- **`Go` at the last cue** reports the cue and changes nothing.

The engine clock is explicit: `Engine::tick(now)` advances playback, and the
output thread calls it at the engine tick rate (60 Hz by default). A test drives
time by handing `tick` its own `Instant`s, so fade behaviour is deterministic
and needs no sleeping.

## Command reference (new in Phase 2)

| Command | Meaning |
|---|---|
| `Fixture 1 At ColorAdd_R 40` | set any attribute of the mode |
| `At Red Full` | pretty attribute names work |
| `At Palette Color 3` | apply a palette to the selection |
| `Store Palette Color 1 "Warm"` | capture the programmer in a pool |
| `Delete Palette Color 1` | delete one palette |
| `Palette` | list palettes with their stored values |
| `Store Cue 1.5 "Look" Fade 2 Delay 0.5 Sequence 2` | store or update |
| `Delete Cue 1 [Sequence 2]` | delete a cue |
| `Sequence` / `Sequence 1` | list sequences / the cues of one |
| `Assign Executor 1 Sequence 1` | bind a cue list to a fader |
| `Executor 1 At 75` | set the master fader |
| `Executor` / `Executor 1` | list executors / one executor |
| `Go`, `Back`, `Pause`, `Release` | run playback |
| `Clear Values` / `Clear All` | clear programmer values |
| `New [Show] ["name"]` | start an empty show |

Cue numbers may be fractional (`1.5`), stored as an `f64` and kept sorted.
`Store Cue` captures the selection, or every fixture when nothing is selected.

## Show file, version 2

Version 2 adds three optional sections:

```json
{
  "format": "openlight-show",
  "version": 2,
  "palettes": [{
    "feature": "Color", "number": 1, "name": "Deep Blue",
    "values": { "OpenLight Demo Wash": { "ColorAdd_R": 1.0, "ColorAdd_B": 0.3 } }
  }],
  "sequences": [{
    "number": 1, "name": "Sequence 1",
    "cues": [{ "number": 1.0, "name": "Low", "fade": 1.0, "delay": 0.0,
               "levels": { "1": { "Dimmer": 0.2 } } }]
  }],
  "executors": [{ "number": 1, "sequence": 1, "name": "Executor 1", "fader": 0.75 }]
}
```

- A **version 1** show still loads: the new sections default to empty and the
  document is upgraded in memory, then saved as version 2. A future version is
  refused, not guessed at.
- Loading validates the new sections too: pool names, numbers, duplicate
  palettes/sequences/executors, cue numbers (finite, non-negative, unique,
  ascending), fade and delay ranges, levels inside `0..=1`, and executors that
  point at a sequence that exists.
- Cue levels may name a fixture that has since been unpatched; those values are
  simply ignored at render time, so unpatching never makes a show unloadable.

## Evidence

`cargo test --workspace` runs **121 tests**: 11 in `dmx`, 110 in `engine`. No
hardware and no sleeping is required for the timing tests.

New coverage in this phase:

- **levels** — case-insensitive get/set, tracked differences, blending from and
  to zero, overlay, HTP merge, intensity-only scaling.
- **palettes** — pool derivation from GDTF features and from attribute names,
  capture of a whole colour system, per-type resolve, actionable errors.
- **playback** — tracking accumulation, fades between tracking states, delays,
  pause/resume, executor scaling, release, fractional cue numbers.
- **engine** — store and apply palettes, store and update cues, tracked cue 2,
  Go/Back/Pause/Release, executor fader, programmer-vs-playback precedence,
  `Clear Values`, `New Show`, object listings, save/load of a show containing
  palettes, cues and an executor, and every error path.
- **show** — version 1 upgrade, version 2 validation for the new sections.

The milestone script is `examples/phase2-demo.txt`:

```
$ cargo run -p openlight-console -- --output 1=virtual \
    --script examples/phase2-demo.txt --frames 120 --trace
...
Executor 1 → cue 1 "Low" (fade 1s, delay 0s)
Executor 1 → cue 2 "High" (fade 1s, delay 0s)
Palettes: 1 | Sequences: 1 | Executors: 1
Playback Executor 1: cue 2 (complete)
Universe 1: 120 frames, last frame 513 bytes, start code 0x00, 6 non-zero slots
  00 BF FF 00 4D BF FF 00 4D 00 00 …
```

`BF` is 191: cue 2's 100% dimmer through the 75% master fader. `FF` is red and
`4D` is the 30% blue from the palette, for both washes.

## Limitations

Deliberate for this phase:

- **No cue-only or tracking shield.** Every store is tracking; `Cue Only`,
  tracking distance and break are not implemented.
- **No effects, recipes, timecode or auto-follow.** A sequence advances only on
  `Go`/`Back`; a cue does not trigger the next one.
- **One cue list per executor**, no fader pages, no executor buttons other than
  Go/Back/Pause/Release.
- **The master fader scales intensity only**; there is no separate grand master,
  and colour/position faders are not implemented.
- **Palette values are per fixture type**, so a palette stored from one type
  does not apply to another; the command reports the fixtures it skipped.
- **No update/merge semantics.** Storing over a palette or cue replaces it.
- **No move-in-black**, no parameter timing per attribute (fade and delay are
  per cue and apply to every attribute).
- **Release is instant**, and deleting a cue does not stop a playback that is
  already past it.
- **No groups, worlds or filters**, so fixture selection is still an explicit
  fixture list.
- **Playback state is not saved.** Loading a show always starts idle; only cue
  data, palettes and fader positions come back.
- **Text-only views.** Sequence, palette and executor listings are command
  output; there are no sheets or windows yet.
- **The programmer has one value per attribute per fixture.** There is no
  per-user programmer, no blind editing and no undo.

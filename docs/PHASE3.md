# Phase 3 — groups, worlds, filters, masters and effects

Phase 2 gave the desk a programmer, palettes, cues and executor faders. Phase 3
adds the layer that makes a rig manageable: **groups** to address it, **worlds**
and **filters** to fence off parts of it, **masters** to scale it, and
**effects** to animate it. Everything still goes through the same command line
and the same single-writer engine.

## How the layers stack

```
programmer  (wins per attribute)
    ↓
playback    (cue tracking at the current tick)
    ↓
effects     (offset around the merged level, clamped to 0..100%)
    ↓
masters     (grand master × lowest group master, intensity only)
    ↓
DMX frame
```

Each layer is pure: rendering never changes the show, and effects never write
the programmer, so clearing or stopping anything leaves the programmed look
exactly where it was.

## Groups

A group is a numbered, named set of fixtures with its own intensity master.

```
Fixture 1 Thru 4 At 40
Store Group 1 "All"        capture the selection
Group 1                    select its fixtures
Group 1 At 50              set Dimmer on the whole group
Group 1 At Palette Color 2  apply a palette to the group
Group 1 Full / Group 1 Off
Delete Group 1
Group                      list groups, fixtures and masters
Master Group 1 At 50       group master
```

Group membership is stored by fixture number, not by index. Fixtures that have
since been unpatched are skipped when the group is used, so unpatching never
makes a group (or the show) invalid.

**Group masters** scale intensity at the output. A fixture can belong to several
groups; the **lowest** master wins, so pulling one group down dims its members
without silently ignoring the other group. Masters are part of the show file.

## Worlds and filters

A **world** limits which fixtures may be selected and programmed. It never
limits output: a cue that is already running keeps driving the whole rig, which
is the point of a world — fence off the operator, not the show.

```
Fixture 1 Thru 2
Store World 1 "Front"
World 1                    activate
World All                  deactivate
World                      list worlds and which one is active
```

With a world active, a command that reaches outside it fails with the fixture
numbers and the world's name, and stores capture only fixtures inside the world.
`Clear All` is deliberately the exception: it is the panic button and clears
every programmer value.

A **filter** limits which *features* may be programmed. Features are the same
pools palettes use (the head of the GDTF attribute, so `Dimmer` and `Color`).

```
Store Filter 1 "Intensity" Dimmer Color
Filter 1                   activate
Filter All                 deactivate
Filter                     list filters and which one is active
```

While a filter is active:

- setting an attribute outside it is refused;
- applying a palette whose pool is outside it is refused;
- a cue or palette captures only the attributes it allows.

World and filter are independent and can be active together. They are console
state, not show data, so they start off after a load. Filter granularity is the
feature pool, not the individual attribute.

## Masters

| Master | Range | Scales |
|---|---|---|
| Grand | 0–100% | every intensity attribute |
| Rate | 0–400% | effect speed (100% is nominal) |
| Group | 0–100% | intensity of that group's fixtures |

```
Master Grand At 75 | Full | Off
Master Rate At 200
Master Group 1 At 50
Master
```

Masters scale **intensity only**; colour and position pass through untouched, so
a master never changes the look, only its brightness. `Master` lists the grand,
the rate and every group master.

## Effects

An effect (phaser) is a stored waveform that modulates **one attribute** of the
fixtures it is applied to.

```
Store Effect 1 "Pulse" [Attribute Dimmer] [Waveform Sine] [Rate 1] [Size 50] [Spread 0]
Effect 1 On [Fixture 1 Thru 4]     start on the selection, or the given fixtures
Effect 1 Off                       stop it everywhere
Effect Off                         stop every effect
Effect / Effect 1                  list effects and what is running
Delete Effect 1
```

Defaults: `Dimmer`, `Sine`, 1 Hz, 50%, no spread. Waveforms are `Sine`,
`Square`, `Triangle` and `Ramp`; all run one cycle from −1 to +1.

At time *t*, for fixture *i* of a run:

```
phase  = (t − started) × rate_hz × rate_master + i × spread/360
offset = size × waveform(phase)
level  = clamp(base + offset, 0, 1)
```

`base` is the programmer-over-playback value, so an effect **pulses around the
programmed level**. For a full black-to-full sweep, set the base to 100% and the
size to 100% (or 0% base and 50% size for a 0–50% pulse).

An effect is **live**: it runs on top of the show, does not write cues or
palettes, and its running state is not saved with the show. Cues therefore
capture the static look; effects are re-applied by the operator (or a future cue
effect column). Starting an effect that is already running replaces its fixture
list and restarts its phase.

Effects run on the engine clock: `Engine::tick` reports a change on every tick
while an effect is running, so the output thread keeps re-rendering and sending
frames. Tests drive that clock with explicit instants and never sleep.

## Command summary

| Command | Effect |
|---|---|
| `Group 1`, `Group 1 At 50`, `Group 1 Full`, `Group 1 Off` | address a group |
| `Store Group 1 ["name"]`, `Delete Group 1`, `Group` | group pool |
| `Store World 1 ["name"]`, `World 1`, `World All`, `Delete World 1`, `World` | worlds |
| `Store Filter 1 ["name"] <Feature...>`, `Filter 1`, `Filter All`, `Delete Filter 1`, `Filter` | filters |
| `Master Grand At 75`, `Master Rate At 200`, `Master Group 1 At 50`, `Master` | masters |
| `Store Effect 1 [...]`, `Effect 1 On`, `Effect 1 Off`, `Effect Off`, `Delete Effect 1`, `Effect` | effects |

Filter names must be quoted (`Store Filter 1 "Intensity" Dimmer`) so bare words
stay unambiguous feature names.

## Show format v3

The show file is now version 3. New sections are `groups`, `worlds`, `filters`,
`effects`, `grand_master` and `rate_master`; all of them default, so version 1
and version 2 documents still load and are upgraded on read (v1 had no palettes,
sequences or executors either). Validation covers every new pool:

- group: non-zero unique number, no duplicate fixture, master in 0..=1;
- world: non-zero unique number, at least one fixture, no fixture 0;
- filter: non-zero unique number, at least one feature, no duplicate feature;
- effect: non-zero unique number, non-empty attribute, rate 0..=100 Hz,
  size 0..=1, spread 0..=360°;
- grand master 0..=1, rate master 0..=4.

## Evidence

`cargo test --workspace` runs **154 tests**: 11 in `dmx`, 143 in `engine`
(30 more than Phase 2). The new coverage:

- **pools** — group membership and names, world membership, case-insensitive
  filter matching.
- **effect** — every waveform over a full cycle, rate and phase, spread between
  fixtures, rate master scaling, restart semantics.
- **render** — grand and group masters scale intensity only, overlapping groups
  use the lowest master, effects clamp, masters do not scale effect colour.
- **show** — v2 files upgrade to v3 with defaults, every new validation rule.
- **command** — group, world, filter, master and effect grammar and its errors.
- **engine** — group selection and levels, group and grand masters, a world
  blocking control while a cue keeps the rig lit, a filter limiting programmer
  and store, effects animating between ticks, rate master speeding them up,
  world/filter refused for effects, delete stops an effect, save/load of every
  new object, status and error paths.

The demo script ends with a deterministic frame (no effect running):

```bash
cargo run -q -p openlight-console -- --output 1=virtual \
  --script examples/phase3-demo.txt --frames 120 --trace
```

```
Universe 1: 120 frames, last frame 513 bytes, start code 0x00, 4 non-zero slots
  00 BF 00 00 00 BF 00 00 00 BF 00 00 00 BF 00 …
```

`BF` is 75% through the grand master on each fixture's dimmer.

## Known limitations

- **Effects are single-attribute.** A circle phaser needs coordinated pan/tilt
  with a 90° offset, and a rainbow phaser needs hue spread across three colour
  attributes; both are out of this phase. There is no step editor, no effect
  column in cues and no per-executor speed.
- **Rate master is global.** It scales every effect; there are no selected,
  time or timing masters yet.
- **Filters work at feature-pool granularity**, not per attribute, and they
  constrain the programmer and stores, not running playback.
- **Worlds limit control only**; `Clear All` deliberately ignores them.
- **Groups edit by re-store.** There is no `Group 1 Add Fixture 5` command yet,
  and group masters are the only group-level property.
- **Effect output is unipolar-clamped**: at a 0% base the negative half of the
  wave is clipped, which is why the documented full sweep uses a 100% base.
- **No GUI.** Everything here is command line, a sheet and listings.

The Phase 1 and 2 limitations that still apply (no cue-only/tracking shield,
no move-in-black, no undo, instant release, playback state unsaved, text-only
sheets) are unchanged.

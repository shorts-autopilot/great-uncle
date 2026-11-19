# 14 · Effects: BPM, multi-attribute and phase

**Rank** 14 · **Order** 12 · **Size** M · **Show** v11 · **Desk** d1 (additive: tempo) · **Depends on** 03

## Problem

An effect modulates **one** attribute at a **rate in Hz**. That covers a dimmer
pulse and nothing an operator actually wants:

- a circle needs pan and tilt at the same rate, 90° apart;
- a rainbow needs R, G and B at 120° apart;
- anything musical needs to be in **beats**, not hertz, so it follows the tempo
  of the track instead of drifting against it.

Changing tempo today means editing every effect's `rate_hz` by hand and
remembering what "2 Hz" was supposed to mean.

## Behaviour when this is done

```
Tempo 128
Tap Tempo                        three taps set the desk tempo
Store Effect 1 "Circle" Rate 1/4 Attributes Pan Tilt Phase Pan 0 Tilt 90
Effect 1 On Group 1
Store Effect 2 "Rainbow" Rate 1/2 Attributes ColorAdd_R ColorAdd_G ColorAdd_B Phase ColorAdd_R 0 ColorAdd_G 120 ColorAdd_B 240
```

```
Effect 1 "Circle"  Pan+Tilt  sine  1/4 @ 128 BPM  size 50%  spread 0°
Effect 2 "Rainbow" R+G+B     sine  1/2 @ 128 BPM  size 50%  spread 0°
```

A tempo change retimes both without editing them, and the circle stays a circle
because phase is per attribute, not per fixture.

## Design

### Show data (v11)

```rust
pub struct EffectTarget {
    pub attribute: String,
    pub size: f64,        // amplitude 0..=1, default 0.5
    pub phase: f64,       // degrees 0..=360, default 0
}

pub struct Tempo {
    pub bpm: f64,             // 20..=300
    pub division: BeatDivision, // 1/1, 1/2, 1/4, 1/8, 1/16, 1/32, 1/2T, 1/4T…
}

pub struct Effect {
    pub number: u16,
    pub name: String,
    pub waveform: Waveform,
    pub targets: Vec<EffectTarget>,   // replaces attribute + size
    pub tempo: Option<Tempo>,         // Some = musical, None = rate_hz
    pub rate_hz: f64,
    pub spread: f64,
}
```

`targets` replaces `attribute`/`size`; the v10 reader turns
`{attribute, size}` into a single target with phase 0, and `rate_hz` stays
authoritative when `tempo` is `None`. The `attribute`/`size` fields are kept in
the *reader* only (a legacy struct), not in the current `Effect`.

Beats per cycle:

```
cycles_per_second(tempo) = (bpm / 60) / beats_of(division)
```

`1/4` is one beat per cycle, `1/2` is two beats, `1/8` is half a beat; triplets
multiply by 1.5. `beats_of` is a `const fn` table so the conversion is exact and
testable, and a cycle rate of 0 or a non-finite result is rejected at edit time
rather than producing a frozen effect.

### Tempo (desk, additive)

```rust
pub struct Desk {
    pub tempo: f64,            // BPM, default 120
    pub taps: VecDeque<Instant>, // last 4 taps, for `Tap Tempo`
}
```

- `Tempo 128` sets it; `Tap Tempo` (or just `Tap`) averages the intervals of
  the last three to five taps and sets the tempo; taps more than 2 s apart
  restart the average (a new count-in, not a 20 BPM tempo).
- Effects with `tempo: Some` read `desk.tempo` at evaluation time, so a tempo
  change retimes every running effect with no state to update. Effects with
  `tempo: None` are unaffected — the desk tempo is not a global rate master, and
  conflating them would make `Rate 2` ambiguous.
- Tempo is desk state because it belongs to the track being played, not the
  programming.

### Evaluation

`Effects::output(show, desk_tempo, now)` becomes:

```
for run in runs:
  for target in effect.targets:
    rate = if let Some(t) = effect.tempo { cycles(t, desk.tempo) } else { effect.rate_hz }
    rate *= show.rate_master
    phase = elapsed * rate + fixture_index * spread/360 + fixture_phase + target.phase/360
    offset = target.size * waveform.value(phase)
    output.add(fixture, target.attribute, offset)
```

- `fixture_phase` is the run's start phase, unchanged from Phase 3.
- A target whose attribute the fixture does not have is skipped silently; the
  `Effect … ` listing says how many fixtures were skipped (`12 of 24 fixtures
  have no Tilt`), which is the information an operator needs.

### Commands

| Command | Effect |
|---|---|
| `Store Effect 1 "Circle" Attributes Pan Tilt` | create with targets |
| `Effect 1 Attributes Pan Tilt` | replace the target list |
| `Effect 1 Attribute ColorAdd_R Size 30 Phase 120` | edit one target |
| `Effect 1 Rate 1/4` | musical rate (implies a tempo) |
| `Effect 1 Rate 2hz` | free rate (clears tempo) |
| `Effect 1 Tempo Off` | back to Hz at the current derived rate |
| `Tempo 128` / `Tap` | desk tempo |

`Attributes` takes words until the next keyword, which the existing tokeniser
already supports (it lexes words, not keywords). `Rate 1/4` needs one lexer
concession: `/` inside a bare token. That is a two-token lookahead in the number
lexer, which already handles `:` for timecode — the same mechanism, not a new
one.

### Listing

`Effect` listing becomes one row per target, with a header row per effect:

```
Effect 1 "Circle"   Sine  1/4 @ 128 BPM  spread 0°   24 fixtures
  Pan       size 50%  phase 0°
  Tilt      size 50%  phase 90°
```

The `Status` line for a running effect names the effective cycles/second, so
"is it actually in time" is answerable at a glance.

## Implementation plan

1. Add `EffectTarget`, `Tempo`, `BeatDivision::beats`, `cycles_per_second` and
   their tests; bump to v11 with the legacy reader.
2. Rewrite `Effects::output` for targets and tempo; port the existing effect
   tests one-to-one (a single-target effect must produce identical offsets).
3. Add `Desk.tempo`/`taps` (`serde(default)`, so no desk version bump), `Tap`
   handling and the tempo line in `Status`.
4. Add the command arms, the `/` token support, the listing and HELP.
5. Extend `examples/phase3-demo.txt` with a two-target circle and assert the
   bytes at two instants (quarter cycle apart).

## Tests

- **effect** — `1/4` at 120 BPM is 2 cycles/s; `1/2` is 1; `1/8` is 4; a triplet
  is ×1.5; a tempo change retimes a running effect without restarting its phase;
  a two-target effect with 90° phase difference produces orthogonal motion
  (`pan` at max when `tilt` is 0); a skipped attribute does not disturb the
  others; a single-target effect matches the Phase 3 bytes exactly.
- **desk** — taps at 500 ms intervals give 120 BPM; a 3 s gap restarts the
  average; tempo round-trips in the desk file.
- **engine** — `Tempo 140` changes a musical effect's output and leaves a
  `2hz` effect unchanged; `Effect 1 Rate 2hz` clears the tempo; `Attributes`
  rejects an attribute no patched fixture has, naming it; `Tap` with one tap
  does nothing and says so.
- **show** — a v10 effect upgrades to one target with the old size and phase 0;
  an invalid division (`1/3`) is rejected with the list.
- **command** — `Rate 1/4` and `Rate 2hz` both parse; `Phase 400` is rejected.

## Boundaries

- No waveform tables, no custom curves, no random/noise waveform (a deterministic
  desk is easier to rehearse; a `Random` could be added as a seeded waveform
  later).
- No effects on groups/worlds (fixtures only, as today), no effect fades in/out,
  no audio beat detection.
- No MIDI clock input; item 15 handles MIDI timecode, not beat clock.

## Risks

- **Musical vs free rates in one field.** Handled by one authority per effect
  (`tempo: Some` ⇒ derived rate, `rate_hz` ignored) and a listing that always
  prints the effective value; the alternative (two fields that can disagree) is
  what most desks get wrong.
- **Phase arithmetic drift.** Phases are degrees converted once per evaluation;
  the phase test asserts orthogonality at several instants, not just one.
- **More work per tick.** Targets multiply the per-run work; a 4096-fixture
  effect with 3 targets is 12k offsets per tick, which is why the output is a
  flat `BTreeMap` write rather than a list of allocations.

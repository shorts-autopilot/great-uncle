# 02 · Per-attribute timing, curves and stagger

**Rank** 2 · **Order** 6 · **Size** L · **Show** v7 · **Depends on** 01 (run scheduling), 03

## Problem

A cue has one fade and one delay. Everything in it — dimmer, colour, pan, tilt,
gobo — moves over the same time with the same shape. Real programming needs the
opposite: colour in 0.5 s, position in 3 s, the dimmer snapping on at the end;
fixtures staggered across the stage instead of hitting together; an ease-out on
a bump so it does not read as a cut.

`Run` is built around a single `from`/`to`/`progress`, so this cannot be bolted
on without a model.

## Behaviour when this is done

```
Store Cue 2 Fade 3
Cue 2 Timing Color Fade 0.5
Cue 2 Timing Pan Fade 4 Curve EaseOut
Cue 2 Timing Dimmer Delay 2.5 Curve Snap
Cue 2 Stagger 0.2
Sequence 1
```

```text
Cue 2  Fade 3.0s  Stagger 0.2s
  Color          fade 0.5s  delay 0.0s  Linear
  Pan            fade 4.0s  delay 0.0s  EaseOut
  Dimmer         fade 3.0s  delay 2.5s  Snap
```

The colour is in place after half a second, the pan is still gliding at four,
the dimmer snaps at 2.5 s, and each consecutive fixture starts 0.2 s later.

## Design

### Show data (v7)

```rust
pub enum Curve { Linear, EaseIn, EaseOut, EaseInOut, Snap }

pub struct AttributeTiming {
    pub fade: Option<f64>,    // None = the cue's fade
    pub delay: Option<f64>,   // None = the cue's delay
    pub curve: Curve,         // default Linear
}

pub struct Cue {
    // … existing …
    pub timing: BTreeMap<String, AttributeTiming>,  // attribute -> timing
    pub stagger: f64,                                // seconds per fixture
}
```

Keys are canonical attribute names (`Dimmer`, `ColorAdd_R`, `Pan`), resolved
through the mode at edit time exactly like `At`, so `Cue 2 Timing Colour …`
stores `Color`-pool canonical names rather than whatever the operator typed.
`Option` rather than a sentinel means "follow the cue" is representable and a
later cue-fade edit moves every attribute that did not override it.

Validation: times are finite and within `MAX_TIME`, `stagger` within
`0..=MAX_TIME`, no unknown attribute names for any mode that a cue's stored
values use (a cue with no value for an attribute may still carry timing for it —
it applies the next time that attribute appears, and validation allows it).

### The fade plan

`Run` stops carrying `from`/`to` and carries lanes:

```rust
struct Lane {
    fixture: u16,
    attribute: String,
    from: f64,
    to: f64,
    start: Duration,   // cue delay + attribute delay + stagger offset
    fade: Duration,
    curve: Curve,
}
```

`go` builds the lanes once, at cue start:

1. `from` is the target's current levels (or the previous run's output on a
   restart), `to` is `sequence.state_at(index)`.
2. For each `(fixture, attribute)` in `to`, timing is
   `cue.timing[attribute]` if present, else the cue's `fade`/`delay`.
3. `start = cue.delay + attr.delay + stagger * index_of_fixture_in_to`.
   The stagger index is the fixture's position in the sorted fixture list of the
   cue's target state, not its fixture number, so staggering a group of 5, 9, 12
   moves them in that order rather than by number.
4. `fade` is the resolved attribute fade. `Snap` is stored as `Curve::Snap` with
   its fade kept for the record but evaluated as an instant change at `start`.
5. A lane is emitted **only when `from != to`**, so a tracked no-op costs
   nothing. The MIB snap (Phase 4) is folded in before lanes are built: it
   rewrites `from` for non-intensity attributes of dark fixtures, and those lanes
   then carry `fade = 0` because `from == to`.

Evaluation:

```rust
fn lane_value(lane: &Lane, now: Instant, start: Instant) -> f64 {
    let elapsed = now.saturating_duration_since(start);
    if elapsed <= lane.start { return lane.from; }
    let faded = elapsed - lane.start;
    if lane.fade.is_zero() { return lane.to; }
    let t = (faded.as_secs_f64() / lane.fade.as_secs_f64()).clamp(0.0, 1.0);
    lane.from + (lane.to - lane.from) * lane.curve.apply(t)
}
```

Cost per frame is one pass over the lanes and one `Levels` build: the same
order as today's single blend, with no per-frame allocation of plans.

`Curve::apply` is a small, total function on `0..=1`:
`Linear = t`, `EaseIn = t²`, `EaseOut = 1-(1-t)²`, `EaseInOut` is the piecewise
quadratic, `Snap = if t > 0 { 1 } else { 0 }`. Quadratic rather than cubic
because it is cheap, monotone, and visibly different at desk scale.

### Pause, resume and scrub

A paused lane set is frozen by capturing the evaluation once, exactly as Phase 4
does. `resume` rebases every lane with its remaining fade and delay:
`start = 0`, `fade = fade * (1 - t)`, keeping the curve from the current point —
which is what "continue where it stopped" means for an eased lane, and is why
the plan stores the curve rather than a precomputed table.

### Commands

| Command | Effect |
|---|---|
| `Cue 2 Timing Pan Fade 3` | per-attribute fade |
| `Cue 2 Timing Pan Delay 0.5` | per-attribute delay |
| `Cue 2 Timing Pan Curve EaseOut` | curve only |
| `Cue 2 Timing Pan` | show one attribute's timing |
| `Cue 2 Timing` | show the cue's timing table |
| `Cue 2 Timing Pan Off` | drop the override, follow the cue again |
| `Cue 2 Stagger 0.2` | stagger |

One keyword family (`Timing`) with optional clauses, matching
`Store Cue … Fade … Delay …`: each clause is independent, so a script can set
one without restating the others. `Off` is the existing idiom for "remove the
override".

### Listing

`Sequence` gains an indented timing block under each cue that has overrides, and
the `Fade` column shows the cue fade. The block prints only attributes that
differ from the cue, so a plain cue list stays one line per cue.

## Implementation plan

1. Add `Curve`, `AttributeTiming`, `Curve::apply`, the `Cue` fields, validation,
   and the v6→v7 upgrade (both defaults empty/zero).
2. Replace `Run::{from, to}` with `Vec<Lane>`; keep a fast path: if every lane
   has the cue's fade and delay and `Linear`, the old single-blend evaluation is
   used, so a Phase 4 show renders through the same arithmetic it did before.
3. Rewrite `go` to build lanes (including MIB and the stagger index) and
   `levels_at` to evaluate them; port the existing fade tests unchanged, then
   add the lane tests.
4. Rework `pause`/`resume` for lanes; assert that pausing mid-ease and resuming
   finishes at the same `to` values.
5. Add `Cue <n> Timing …` and `Cue <n> Stagger …` parse/dispatch/listing.
6. Extend `examples/phase4-demo.txt` with one per-attribute fade and assert the
   trace shows the colour arriving before the dimmer.

## Tests

- **playback** — one attribute with a 0 s fade is at its target at `delay`
  while another is still moving; a per-attribute delay adds to the cue delay; a
  stagger offsets consecutive fixtures by exactly `stagger`, in target-state
  order; `Snap` is at `from` before its start and at `to` after; `EaseOut` is
  ahead of linear at the midpoint and equal at both ends; lanes for unchanged
  values are not emitted; a whole-cue `Linear` fade evaluates bit-identically to
  Phase 4.
- **engine** — a per-attribute timing round-trips through save/load; `Timing
  Pan Off` restores the cue fade; timing for an attribute the fixture lacks is
  rejected naming the attribute; timing a cue that is currently running does not
  disturb the run.
- **show** — v6 upgrade yields empty timing and zero stagger; a negative stagger
  or a NaN fade is rejected.
- **command** — every clause combination parses; `Curve Bounce` is rejected with
  the list of curves.

## Boundaries

- No per-fixture timing overrides (only per attribute plus stagger), no
  per-attribute stagger, no delay curves.
- No fan/curve shapes (item 14's effects cover oscillation; fans are a GUI
  feature).
- No "copy timing from cue" command; `Timing` is small enough to retype, and
  item 10's `Update` is where bulk editing belongs.

## Risks

- **Lane count.** A cue touching 4096 fixtures × 8 attributes is 32k lanes. Each
  lane is ~60 bytes, so ~2 MB per run — fine for one desk, but `scrub` and
  `pause` clone levels per frame. Mitigation: lanes borrow attribute names from
  the `to` map instead of owning `String`s, and `levels_at` writes into one
  reused `Levels` where the API allows it.
- **Evaluation change risk.** The fast path exists so a Phase 4 show cannot
  regress numerically; the test that asserts bit-identical output is not
  optional.
- **Semantics of `Snap` with no fade.** Treated as "instant at start"; a user
  expecting "instant at the end of the cue" gets the documented behaviour, and
  spelling it `Delay` makes it explicit.

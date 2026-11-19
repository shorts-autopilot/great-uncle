# 01 · Cue triggers and executor state

**Rank** 1 · **Order** 5 · **Size** L · **Show** v6 · **Desk** d1 (additive: executor state) · **Depends on** 03

## Problem

A cue list only advances when someone types `Go`. The only automatic playback
is timecode, which needs a whole second document and a track. That leaves the
common case — a five-cue list that should follow itself — without an answer, and
it makes the desk useless for anything an operator wants to walk away from.

Executors are half a feature: `Executor { number, sequence, name, fader }` has a
master but no button, no flash, no solo, and its position lives in the show file
where a fader does not belong.

## Behaviour when this is done

```
Store Cue 1 Fade 2
Store Cue 2 Follow 1 Fade 3      after cue 1's fade ends, wait 1 s, start cue 2
Store Cue 3 Time 2 Fade 4        start 2 s after cue 2 started (overlapping)
Store Cue 4 Fade 1               waits for Go
Sequence 1
```

```text
Cue  Name   Fade  Delay  Trigger        Mib  Values
1          2.0s   0.0s  Go             -    4
2          3.0s   0.0s  Follow 1.0s    -    2
3          4.0s   0.0s  Time 2.0s      -    2
4          1.0s   0.0s  Go             -    2
```

```
Executor 1 Mode Fader
Executor 1 At 60                 the cue crossfades manually, in step with the fader
Executor 2 Mode Flash
Executor 2 Flash                 while held, that list runs at full
```

## Design

### Show data (v6)

```rust
pub enum Trigger { Go, Follow, Time }

pub struct Cue {
    // … existing fields …
    pub trigger: Trigger,   // default Go
    pub trigger_time: f64,  // seconds; only meaningful for Follow and Time
}

pub enum ExecutorMode { Go, Fader, Flash, Solo }

pub struct Executor {
    pub number: u16,
    pub sequence: u16,
    pub name: String,
    pub mode: ExecutorMode,   // default Go
}
```

`Trigger` is an enum, not a string, so an unknown trigger is a deserialization
error rather than a cue that never fires. `trigger_time` is validated to
`0..=MAX_TIME` and is ignored (but preserved) on a `Go` cue, so switching a
trigger back and forth does not lose the number.

### Playback runs

A `Run` learns when it began and when it became stable:

```rust
pub struct Run {
    // … existing …
    pub started: Instant,
    pub settled: Option<Instant>,   // set once progress reaches 1.0
    pub held: bool,                 // Fader-mode run driven by a hand
}
```

`Playback` gains one method that answers "what should start next", and it takes
`now` explicitly so tests never sleep:

```rust
pub fn due(&self, show: &Show, now: Instant) -> Vec<(Target, u16, usize)>;
```

For every run whose `settled` is set and whose cue is not the last one, it looks
at the *next* cue's trigger:

| Trigger | Due when |
|---|---|
| `Follow t` | `now >= settled + t` |
| `Time t` | `now >= started + t` |
| `Go` | never |

A run with a zero fade settles on the tick it starts, so `Follow 0` chains
immediately — the same rule the timecode player uses for an event at frame 0.
`Engine::tick` calls `due` before `refresh` and starts each returned cue with
`go`, exactly as `fire_timecode_event` does, so a due cue is already in the
frame. A paused run does not settle and therefore never triggers.

**Loops and guards.** A `Follow` chain on the last cue does nothing; there is no
wrap. Because every trigger is forward-only and `Follow` requires a settled
fade, a chain cannot loop. `Time` on a cue with a fade longer than its trigger
time overlaps, which is the point; the previous run is replaced by `go` for that
target, so a fast chain cannot accumulate runs.

**Why not in the cue list object.** Putting due-time computation in `Engine`
would make it untestable without a clock; putting it in `Playback` keeps it a
pure function of `(show, now)` plus the runs it already owns.

### Executor state (desk, additive)

```rust
pub struct ExecutorDesk {
    pub fader: f64,     // 0..=1, the live position
    pub flash: bool,
    pub solo: bool,
}

pub struct Desk {
    pub executors: BTreeMap<u16, ExecutorDesk>,
    // … the fields from 03 …
}
```

Running cues stay in `Engine::playback` and are projected into `Desk.runs` when
saving (see 03), so a desk file records *which cue* each target is on without
duplicating the fade machinery.

- **Go**: a press starts the next cue; the fader is a master, as today.
- **Fader**: the run's position follows the fader. At 0 the cue contributes its
  `from` state, at 1 its `to` state. Implemented with the existing paused-run
  machinery: `Playback::scrub(target, fraction)` sets `paused` to the blended
  levels and `paused_progress` to the fraction, so pause/resume, release and
  output merge need no new code. Releasing the fader does not move the cue.
- **Flash**: while `flash` is true, that executor's contribution is scaled to
  1.0 regardless of fader; releasing restores the fader. Flash on an unstarted
  list starts cue 1.
- **Solo**: while any executor is soloed, every *other* playback's intensity is
  scaled by 0 at the merge step. Solo is a merge-time flag, not a state change,
  so a solo never alters a run.

`Masters` gains `executor:<n>` derived from `fader`, `flash` and `solo` in one
place (`Desk::executor_master(n) -> f64`), so rendering has one expression for
"how loud is this executor" instead of three call sites.

### Commands

| Command | Effect |
|---|---|
| `Store Cue 2 Follow 1.5` | store/retime with a follow trigger |
| `Store Cue 2 Time 0.5` | store/retime with a time trigger |
| `Cue 2 Trigger Go` | set the trigger without re-storing values |
| `Cue 2 Trigger Follow 1` | idem, with a time |
| `Executor 1 Mode Fader` | set the button behaviour |
| `Executor 1 Flash` / `Executor 1 Flash Off` | hold/release flash |
| `Executor 1 Solo` / `Executor 1 Solo Off` | hold/release solo |

`Cue <n> Trigger …` is a show edit (`mutate`, undoable). `Executor … Flash` is
a desk action (no undo step) for the same reason a fader move is not.

### Listings

`Sequence` gains a `Trigger` column formatted `Go`, `Follow 1.0s`,
`Time 2.0s`. `Executor` gains `Mode` and `State` (`idle`, `cue 2 running`,
`flash`, `solo`). Both are built by the same functions the shell uses, so the
terminal windows, the reports in item 11 and any future GUI cannot drift.

## Implementation plan

1. Add `Trigger`, `ExecutorMode`, `Cue.trigger`, `Cue.trigger_time`,
   `Executor.mode`; serde defaults `Go`; validate times and enum membership;
   bump to v6 and add the v5→v6 upgrade (both default to `Go`).
2. Extend `Run` with `started`, `settled`, `held`; set `settled` in
   `Playback::refresh` when progress reaches 1.0 and the run is not paused.
3. Add `Playback::due`, `Playback::scrub`; write their unit tests against
   explicit instants before wiring anything.
4. Add `ExecutorDesk` to `Desk`, `Desk::executor_master`, and route
   `Playback::refresh`'s fader lookup through the desk.
5. Call `due` from `Engine::tick`; start each returned cue through the same
   path timecode uses, and factor that path into `Engine::start_cue`.
6. Add the parser arms, dispatch, listing columns and HELP lines.
7. Extend `examples/phase4-demo.txt` with a `Follow` chain and assert the trace
   still ends at the same look.

## Tests

- **playback** — `Follow` fires only after the fade settles; `Follow 0` on a
  zero fade chains on the same tick; `Time` fires from the cue start and
  overlaps; a `Go` cue waits forever; the last cue never wraps; a paused run
  never triggers; `scrub` at 0.5 equals the blend at half the fade.
- **desk** — executor master combines fader, flash and solo; solo silences the
  other executor's output and restores it on release; the desk round-trips
  executor state.
- **engine** — a three-cue follow chain runs with no commands between ticks; a
  timecoded cue and a follow cue on the same sequence do not double-start;
  `Cue 2 Trigger Follow 1.5` is one undo step; a trigger edit does not restart a
  running cue; `Executor 1 Mode Fader` round-trips.
- **command** — every trigger form parses; `Trigger Follow` without a time and
  `Trigger Follow -1` are rejected with messages naming the value.
- **show** — v5 files upgrade with `Go` triggers; an unknown trigger name is a
  deserialization error.

## Boundaries

- No `Sound`/audio trigger (it needs a beat detector; item 14 brings a tempo
  clock, not an input).
- No cue-level `Loop`, `Jump` or `Goto` — a forward-only chain keeps a cue list
  impossible to wedge.
- No multiple cues per executor, no pages, no executor macros, no fader curves.
- `Time` and `Follow` are not compensated for the rate master.

## Risks

- **Tick-driven chains depend on the tick.** A 60 Hz engine tick means a follow
  can fire up to 16 ms late. Acceptable for a lighting desk and documented; the
  engine tick is already the clock for timecode.
- **Two automatic sources on one target.** Timecode and a chain can both start
  cues on sequence 1. The engine keeps last-writer-wins per target, and
  validation does not forbid it — the alternative (locking a sequenced to one
  source) removes a legitimate workflow.
- **Solo is a foot-gun on a live rig.** It is desk state, never saved into the
  show, and `Status` names the soloed executor so an empty stage is diagnosable
  in one command.

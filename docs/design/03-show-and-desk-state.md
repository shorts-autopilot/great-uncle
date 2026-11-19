# 03 · Show data vs desk state

**Rank** 3 · **Order** 1 · **Size** L · **Show** v5 · **Desk** d1 · **Depends on** nothing

## Problem

`Fixture.levels` is the programmer, and it is serialized into the show file.
That one decision makes four things wrong at once:

- **Save writes the programmer.** A show saved mid-edit reopens with a look the
  operator never stored. The Phase 1 demo show proves it: fixtures 1 and 2 carry
  `Dimmer 1.0` because the demo typed `At Full`, not because a cue stored it.
- **Undo has the wrong granularity.** `History` snapshots the whole `Show`, so
  programmer edits and cue edits share one stack, and a cue store cannot be
  undone without also reverting the values it captured.
- **Autosave (04) would bake the bug in.** Any journal or recovery design that
  starts from "save the show" inherits programmer-in-show.
- **Nothing owns the handles.** `grand_master`, `rate_master`, `Group.master`
  and `Executor.fader` are fader positions stored as if they were programming.

## Behaviour when this is done

```
Fixture 1 At 50
Save "look.olshow"
New
Load "look.olshow"
Fixture 1                      values: -            (the programmer was not saved)
Sequence 1                     cue 1 has Dimmer 50  (the cue was)
```

- Editing the programmer never changes the `.olshow` file.
- `Save` writes the show and the desk next to it; reopening puts the programmer,
  selection and faders back where they were.
- `Undo` restores the programmer and the show together, still one stack.
- A show opened on another machine without its desk file starts clean, not
  broken.

## Design

### Ownership

`Desk` (`crates/engine/src/desk.rs`) owns everything that is a handle or a
transient decision:

```rust
pub struct Desk {
    pub selection: Vec<u16>,
    pub programmer: Levels,
    pub world: Option<u16>,
    pub filter: Option<u16>,
    pub masters: Masters,      // grand, rate, group:<n>, executor:<n>
    // Saveable projections of runtime state: written on save, applied on load.
    // The live objects stay in Engine (see below); a desk file records where
    // the handles are, not how the fades are currently interpolating.
    pub runs: Vec<RunState>,
    pub effect_runs: Vec<EffectState>,
    pub timecode: Option<TimecodeState>,
}
```

`Show` keeps exactly what a different desk would need to reproduce the
programming: types, universes, fixtures (number, name, type, mode, patch,
`home` later), palettes, sequences, executors (number, sequence, name), groups
(number, name, fixtures), worlds, filters, effects, timecodes. It loses
`Fixture.levels`, `grand_master`, `rate_master`, `Group.master` and
`Executor.fader`.

Masters move because they are physical positions: they belong to the desk in
front of you, not to the document, so a fader nobody touched must not jump when
a show loads. Programming is the opposite: it has to travel with the file.

### Engine

```rust
pub struct Engine {
    pub show: Show,
    pub desk: Desk,
    // Session state: live, never serialized. The desk holds the projection.
    pub playback: Playback,
    pub effects: Effects,
    pub timecode: TimecodePlayer,
    pub revision: u64,
    pub status: Arc<Mutex<OutputStatus>>,
    history: History,
    label: String,
    clock: Instant,
}
```

`render` keeps taking the pieces it needs rather than the containers —
`render(show, programmer, playback_levels, effects, masters)` and the same for
`Stage::new` — so rendering is still testable without building a desk. The
programmer now comes from `desk.programmer`; playback levels still come from
`Playback::output()`.

`set_level`, `clear`, `off` and `apply_palette` write `desk.programmer` and do
**not** call `mutate`: they change no show data. They still bump `revision` so
the output thread re-renders. `store_cue` reads `desk.programmer` where it read
`fixture.levels`, and captures through the filter exactly as today.

`mutate` keeps one choke point for show edits and records `Snapshot { show,
programmer }` — the programmer is part of undo because "put that back" usually
means the values as well as the cue, but only the programmer, never the running
playback.

### Desk file

```
openlight-desk v1, JSON, one file per show
<show path>.oldesk            e.g. "look.olshow" -> "look.oldesk"
```

```json
{
  "format": "openlight-desk",
  "version": 1,
  "show": "look.olshow",
  "selection": [1, 2],
  "programmer": { "1": { "Dimmer": 0.5 } },
  "world": null,
  "filter": null,
  "masters": { "grand": 0.75, "group:1": 1.0 },
  "runs": [{ "target": { "sequence": 1 }, "cue": 2.0, "paused": false }],
  "effect_runs": [], "timecode": null
}
```

- `show` is the file name the desk was written for. A mismatch is a warning in
  `Load`, not a refusal: a moved show keeps its desk.
- Desk state is validated with the same rule as the show — a stale fixture
  number or a target that no longer exists is dropped with a warning, never an
  error, because a desk file is disposable.
- A desk is written atomically (item 04's writer) and only by `Save`,
  `Save Desk <file>`, autosave and `--desk <file>` at exit.
- `Save` writes the **desk first, then the show**. If the desk write fails the
  show is untouched, so a failure can never leave a show whose desk claims a
  newer state than the document has.

### Commands

No new grammar. Three existing commands gain precision:

| Command | Before | After |
|---|---|---|
| `Save [file]` | show only | show, then its desk beside it |
| `Save Show <file>` | — | show only, e.g. for handing a show to another desk |
| `Save Desk <file>` | — | desk only |
| `New [name]` | new show, keep programmer | new show **and** a fresh desk |
| `Load <file>` | replaces the show | replaces show and desk; `Load Show <file>` keeps the desk |

### Upgrade from v4

The v4 reader collects `Fixture.levels` into `desk_seed.programmer` and the
three master fields into `desk_seed.masters`. `Engine::load` applies the seed
only when no `.oldesk` exists, so an old show opens looking exactly as it did,
and the first save writes the same look into the new desk. The seed is reported:
`Upgraded from show v4: programmer and master positions moved to the desk`.

## Implementation plan

1. Add `desk.rs` with `Desk`, `Masters`, `DeskFile`, `save`, `load`, validation
   and its own tests; keep the serialized shape flat so a hand edit is readable.
2. Remove the four field groups from `Show`; bump to v5; make `Show::load`
   return `(Show, LoadReport)` and teach the v1–v4 reader to fill
   `desk_seed`; update `validate`, `Show::new` and every struct literal.
3. Move `selection`, `world`, `filter`, `programmer`, `playback`, `effects`,
   `timecode` out of `Engine` into `Desk`; change every command body that
   touched them; keep `mutate` for show edits only.
4. Thread `&Desk` through `render`, `Stage` and the listing/status functions;
   the sheet now reads the programmer from the desk, the playback window from
   `desk.playback`.
5. Change `History` to `Snapshot { show, programmer }`; `undo`/`redo` swap both.
6. Rewire `Save`/`Load`/`New`, `--desk`, and the shell's status line.
7. Fix the examples: `examples/phase1-demo.txt` no longer saves a programmer
   into its show; the demo assertions stay about output, not the file.

## Tests

- **desk** — round-trip; a dangling fixture number and a missing target are
  dropped with a warning; a show-name mismatch warns and still loads.
- **show** — v4 upgrade moves the programmer into the seed and leaves an empty
  programmer on the `Show`; v5 save has no `levels` key and still round-trips.
- **engine** — `At 50; Save; New; Load` leaves the programmer empty and the cue
  intact; `Save Show` omits the desk; `Load Show` keeps the desk; undo restores
  the programmer and the cue in one step; clearing values is undoable and does
  not change the file.
- **render** — unchanged behaviour with a programmer passed in; masters come
  from the desk.

## Boundaries

- No multi-desk or per-user state, no show locking, no `.oldesk` merge.
- Programmer timing, tracked/untracked flags and "programmer is now a cue"
  workflows are item 10, not this one.
- The show file keeps `executors[].name`; only the fader position moves.

## Risks

- **Largest refactor in the set.** Nearly every `Engine` method touches one of
  the moved fields. Mitigation: move one field group per commit-sized step, with
  the workspace compiling and the existing 184 tests green after each step.
- **Silent upgrade behaviour.** The v4 seed is the only place a user's saved
  programmer migrates; the load report must say so out loud.
- **`Stage` borrows grow.** Passing `&Desk` into rendering couples render to
  desk. Mitigation: `Stage::new(show, programmer, playback, effects, masters)` —
  it takes the pieces, not the container, so render stays testable without a
  desk.

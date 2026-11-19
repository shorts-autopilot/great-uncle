# 10 · Cue-editing safety

**Rank** 10 · **Order** 8 · **Size** L · **Show** v9 · **Depends on** 02 (timing), 03

## Problem

Cues store **tracked deltas**, which is correct and is also how a single edit
silently changes four other cues. Today there is no way to:

- **update** a cue from the programmer without retyping the whole list;
- **block** tracking at a cue so a later cue starts from a known state;
- **shield** an attribute so an upstream edit does not ripple into it;
- **move** or **copy** a cue without recomputing deltas by hand;
- **replace** a cue instead of merging into it;
- delete a cue safely when a timecode event or another cue's tracking depends on
  it.

The existing `Store Cue` re-captures the programmer and writes a delta against
the *previous* cue, so re-storing cue 2 after editing cue 1 rewrites cue 2's
relationship to cue 1 — usually not what was intended.

## Behaviour when this is done

```
Cue 1 At 50
Cue 2 At Full
Update Cue 2                    captures the programmer into cue 2 only
Cue 3 Block                     cue 3 asserts the tracked state at that point
Cue 3 Shield Pan                later edits to cue 1 do not move cue 3's pan
Store Cue 4 Replace             the cue holds exactly what is in the programmer
Cue 2 Move 5                    renumber, and tracking stays correct
Delete Cue 3                    refuses while a timecode event targets it
```

## Design

### Show data (v9)

```rust
pub struct Cue {
    // … existing …
    /// Stop tracking here: the cue stores the complete state, not a delta.
    #[serde(default)]
    pub blocked: bool,
    /// Attributes asserted at a fixed value here: upstream edits do not change
    /// them. The value is stored, because a flag alone cannot survive the very
    /// upstream edit it is meant to block.
    #[serde(default)]
    pub shield: BTreeMap<String, f64>,
}
```

### Rebase: the one operation everything else uses

The reason move/copy/delete/insert are dangerous today is that they reorder cues
whose `levels` are deltas against their predecessor. One function fixes all of
them:

```rust
impl Sequence {
    /// Rewrite every cue's stored levels so the accumulated state at each cue is
    /// unchanged, respecting blocked cues and shields.
    pub fn rebase(&mut self);
}
```

Algorithm, single pass, O(cues × values):

1. Walk cues in order, accumulating `state`.
2. For cue *n*: `absolute = if blocked { cue.levels } else { state.overlay(&cue.levels) }`.
   For blocked cues the stored levels *are* the absolute state.
3. Apply shields: `absolute.set(fixture, attribute, shielded_level)` for every
   entry in `cue.shield` and every fixture the attribute applies to. The stored
   level wins over whatever the accumulated state says, which is exactly what
   "asserted here" means and what makes the shield survive an upstream edit.
4. Store back `cue.levels = absolute.difference(&state_before_this_cue)`.
5. `state = absolute`.

`rebase` is called after `Move`, `Copy`, `Delete`, `Block`/`Unblock` and a
`Store … Replace`; `Store` alone (merge) does not need it, because it already
computes the delta against the right predecessor.

**`Unblock`** clears the flag and rebases, which turns the absolute cue back
into a delta against its predecessor — the edit is reversible.

### `Update`

```
Update [Cue <n>] [Sequence <n>]
```

- Captures the programmer (through the filter, as `Store` does) and writes it
  into the **named** cue, defaulting to the cue currently running on that
  sequence.
- The new `cue.levels` are the programmer's values for the touched attributes,
  merged into the cue's existing delta: attributes the operator did not touch
  keep their tracked values. This is the "merge by attribute" behaviour every
  desk has, and it is why `Update` is separate from `Store`.
- `Update` on a blocked cue writes the absolute state (the cue stays blocked).
- One undo step; message names the cue and how many values changed.

### `Store … Merge|Replace`

- `Merge` (the new default, and a deliberate behaviour change): the programmer's
  attributes are written into the cue and **everything else the cue already
  stored stays**. Re-storing cue 2 after touching only its dimmer keeps its
  colour, which is what "merge into this cue" means everywhere else on the desk.
- `Replace`: the cue's levels become exactly the programmer's delta against the
  previous cue, dropping stored values the programmer no longer has. This is
  today's behaviour, kept as the explicit form for "make this cue match the
  programmer".
- Storing into a blocked cue writes the **absolute** state
  (`state_before.overlay(programmer)`) and leaves the cue blocked; a blocked cue
  has no delta to merge into.
- The change lives in `store_cue`, is one undo step, and the message says
  `Stored`, `Updated (merge)` or `Updated (replace)` so a script's effect is
  visible in the log.

### Block and shield

- `Cue <n> Block` materialises the state at that cue and sets `blocked`. Later
  edits to earlier cues no longer change it, because its own levels are the
  whole look.
- `Cue <n> Shield <attribute>` records the attribute's **current accumulated
  value at that cue** and stores it as the assertion; `Cue <n> Shield
  <attribute> Off` removes it. Because the value is stored, the shield holds
  through any later upstream edit, which is the whole point.
- `Cue <n> Shield` lists attribute and asserted level, so a shield is never an
  invisible flag.
- Both are show edits: one undo step each, and `Cue <n>` listing shows
  `blocked` and the shield list.

### Move, copy, delete

| Command | Notes |
|---|---|
| `Cue 2 Move 5` | refuses if 5 is taken; renumbers; `rebase` |
| `Cue 2 Copy 5 [Sequence 2]` | deep copy including timing and shield; `rebase` on both sequences when it lands elsewhere |
| `Delete Cue <n>` | refuses when a timecode event targets it, naming the timecode and position; otherwise `rebase` |

The delete refusal is a pre-check, not a validation failure: today the command
would fail with "command would leave the show invalid", which does not say
*why*. The pre-check produces `cue 3 is started by timecode 1 at 00:01:30:00;
remove the event first`, which is actionable.

Also: `Cue <n> Show` prints the cue's *absolute* state (what it looks like when
it runs), which is the question an operator actually has and is not derivable
from the delta by eye.

## Implementation plan

1. Add `blocked`/`shield`, validation (shield attributes must exist in at least
   one mode used by the cue, sorted/unique), v8→v9 upgrade.
2. Implement `Sequence::rebase` and `state_at` handling of blocked cues; write
   the property test first: for random sequences, `rebase` then `state_at` is
   identical for every index.
3. Rewrite `Store`'s merge/replace paths on top of `rebase` and
   `difference`; keep the existing tracking tests green.
4. Add `Update`, `Block`, `Unblock`, `Shield`, `Move`, `Copy`, `Show` parse and
   dispatch; route structural edits through `rebase`.
5. Add the delete pre-check for timecode and marker references.
6. Extend `Sequence` listing with block/shield markers and the absolute-state
   view.

## Tests

- **playback** — `rebase` preserves `state_at` for every index across move,
  copy, delete and block; a blocked cue absorbs a later upstream edit; a shield
  holds its stored level while every other attribute ripples, and removing the
  shield lets the upstream value through; `Unblock` restores delta behaviour;
  replace drops attributes the programmer no longer has.
- **engine** — `Update Cue 2` merges only touched attributes and is one undo
  step; `Update` with no running cue errors with a hint; re-storing a cue after
  touching one attribute keeps its other values and `Replace` drops them;
  storing into a blocked cue keeps it blocked and absolute; `Move` onto an existing cue is refused naming it;
  `Copy` to another sequence carries timing and shield; `Delete Cue` names the
  timecode event that blocks it; `Cue 2 Show` prints the absolute state.
- **show** — v8 upgrade yields `blocked: false` and an empty shield map; a
  shield naming an attribute no mode has is rejected; save/load round-trips
  both fields, values included.
- **command** — grammar for every new form; `Cue 2 Shield` with no attribute is
  rejected.

## Boundaries

- No cue-level `Assert`/`Release` (blackout-style per-cue overrides) — that is a
  different console idiom and would double the tracking model.
- No multi-cue edit (`Cue 1 Thru 4 Block`) in this item; the command fits later
  without changing the model.
- No cue list versioning, no undo of playback position.

## Risks

- **Rebase is the whole item.** If it is wrong, moves silently change looks.
  Mitigation: the property test compares every cue's absolute state before and
  after for generated sequences, which is the strongest cheap guarantee here.
- **Shield semantics are the least standard part.** The design defines it
  narrowly ("assert here, do not follow upstream") and prints it in the listing,
  so an operator can see it; a wider definition would need its own design.
- **`Update` on a tracked cue grows the cue.** Every merge adds values, so a cue
  list can become fully tracked over time. That is what tracking is; the
  listing's value count makes it visible.

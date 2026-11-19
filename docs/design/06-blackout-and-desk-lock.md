# 06 · Reversible blackout and desk lock

**Rank** 6 · **Order** 4 · **Size** S · **Show** — · **Desk** d1 (additive: blackout, lock) · **Depends on** 03

## Problem

Two things every desk has and this one does not:

- **A blackout** that does not destroy the look. Today the only way to darken the
  rig is `Clear Values` plus `Off`, which edits the programmer and (before item
  03) the show file. A blackout must be a layer that can be lifted in one
  command and leaves no trace when it is.
- **A lock** for the moments when the desk must not change: during a rehearsal
  line, while the operator is away, while a visiting programmer is looking at
  the screen. Currently a stray keystroke is a show edit.

## Behaviour when this is done

```
Blackout                        rig goes dark, everything else is untouched
Status                          Blackout: on (fade 0.0s) | Lock: off
Blackout Fade 2                 silent 2 s fade to black
Blackout Off                    the same look returns
Lock 4711                       desk locked: programming commands are refused
Fixture 1 At 50                 error: the desk is locked; `Unlock 4711`
Unlock 4711
```

## Design

### Blackout

Blackout is a **render-stage master**, not an edit:

- `Desk.blackout: Option<BlackoutState>` where the state is `{ level: f64,
  started: Instant, fade: Duration }`. A fade ramps a master from 1 to 0 over
  `fade`, driven by the engine tick (like every other timed value, it is a pure
  function of `now`, so tests never sleep). Both fields are `serde(default)`,
  so the desk file needs no version bump.
- In `render`, after the intensity masters and before patch conversion:
  `level *= desk.blackout_master(now)` for intensity attributes only. Colour,
  position and gobo stay put, so lifting blackout restores the exact look with
  no re-fade and no DMX bump on colour.
- `Blackout` sets the master to 0 immediately (`fade 0`); `Blackout Off` ramps
  back to 1 over the same fade or instantly when the fade is zero.
- It never touches the programmer, never records undo, and is saved in the desk
  so a crashed desk comes back dark rather than unexpectedly lit.
- `Status` reports it, and the shell header shows `BLACKOUT` in the title line.

**Why not a grand master to zero?** Because the operator's grand master position
is a value they set and expect to find. Blackout must not overwrite a handle.

### Desk lock

- `Desk.lock: Option<String>` — `None` is unlocked, `Some(code)` is locked. The
  code may be empty (`Lock`), in which case `Unlock` needs no argument.
- `Engine::execute` parses the line, then consults a single allow-list before
  dispatching a *mutating* command:

  ```
  allowed while locked: Unlock, Help, Status, Sheet,
                        every listing (Palette, Sequence, Executor, Group,
                        World, Filter, Effect, Timecode), Blackout Off
  refused:              everything that changes the show, the desk programmer,
                        or the open document
  ```

  Refusal is a `CommandError` with the hint ``Unlock <code>`` and the line is
  *not* journaled.
- The allow-list is a method on `Command` (`fn mutates(&self) -> bool`), not a
  list of match arms in `execute`, so a new command is either classified by the
  exhaustive match (and the compiler complains) or it is a listing. This is the
  design's point: the lock must not silently miss a future command.
- `Blackout Off` is allowed: refusing to lift a blackout while locked is a
  safety hazard.
- The code is compared in constant time and stored as plaintext in the desk
  file. It is a **lock, not authentication** — anyone with the file can read it,
  and the docs say so. There is no recovery beyond editing the desk file, which
  is the correct last resort.

### Commands

| Command | Effect |
|---|---|
| `Blackout [Fade <s>]` | start a blackout, optionally faded |
| `Blackout Off` | lift it |
| `Lock [code]` | lock the desk |
| `Unlock [code]` | unlock; wrong code is an error |

All four are desk actions: no undo step, journaled only as desk changes (item
04) so a recovered session knows the desk was locked.

## Implementation plan

1. Add `BlackoutState` and `Desk::blackout`, `Desk::lock`, `Desk::blackout_master(now)`;
   include both in the desk file with defaults.
2. Thread the blackout master through `Stage` (one multiplication next to the
   existing `intensity_master`), and add the `BLACKOUT` marker to the shell
   header and `Status`.
3. Add `Command::{Blackout, Lock, Unlock}` with the grammar above.
4. Add `Command::mutates()` and gate `dispatch` on it; write the lock tests
   against one command per category so a new category fails loudly.
5. Update HELP, the shell footer and `docs/PHASE4.md`'s status example.

## Tests

- **render** — blackout zeroes every intensity and leaves colour, position and
  gobo bytes unchanged; blackout on top of a grand master of 0.5 produces 0;
  a faded blackout at its midpoint is half intensity; lifting it restores the
  pre-blackout frame exactly.
- **desk** — blackout and lock round-trip; a desk written while blacked out
  reopens blacked out.
- **engine** — `Blackout; Undo` does nothing (no undo step); blackout survives a
  `Fixture 1 At 50` made while dark; `Lock` refuses `Store Cue`, `At`, `Load`,
  `New`, `Patch`, `Delete Cue` with the same hint; listings and `Status` still
  work; `Unlock` with the wrong code fails and leaves the desk locked; `Blackout
  Off` works while locked.
- **command** — `Blackout Fade -1` and `Lock a b` are rejected.

## Boundaries

- No blackout on a subset (worlds/filters do that), no "blackout follows
  timecode", no remote blackout button beyond item 12's mapping.
- No user accounts, no password hashing, no lock timeout, no per-window lock.
- No `Freeze`/`Hold` (a different feature: stop playback without darkening).

## Risks

- **Blackout hides a bug.** An operator can black out and forget a stuck
  channel. Mitigation: `Blackout` is reported in `Status` and the shell header
  every time the desk is looked at.
- **Lock allow-list drift.** Handled structurally by `Command::mutates()` and a
  test per category; the exhaustive match means a new command does not compile
  until it is classified.

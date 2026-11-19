# 09 · Blind, preview and highlight

**Rank** 9 · **Order** 9 · **Size** M · **Show** — · **Desk** d1 (additive: blind, highlight, preview) · **Depends on** 03

## Problem

Every edit is visible. Loading a cue, adjusting a colour, checking what a cue
looks like — all of it goes to the rig, which on a live stage is the one thing
you must not do. There is also no way to answer "what does cue 14 actually
contain" without running it, and no way to find which fixture you are editing
when the rig is dark.

## Behaviour when this is done

```
Blind
Fixture 1 At ColorAdd_B 100      nothing changes on stage
Store Cue 3                      stored from the blind programmer
Blind Off                        the stage takes the change (one frame)
Preview Cue 14                   the shell shows cue 14's output; nothing is sent
Highlight                        selected fixtures go to full so you can see them
```

## Design

### Blind

```rust
pub struct Desk {
    // … existing …
    pub blind: bool,
}
```

- `Stage` receives the programmer **or an empty `Levels`** depending on
  `blind`. That is the whole mechanism: one `if` at the single place where the
  programmer enters rendering, so no command needs a blind variant.
- Commands still run normally: `At` writes the programmer, `Store` captures it,
  `Update`/`Store Cue` work, listings show the blind values. Only output is
  suppressed.
- `Blind Off` makes the programmer live again — the change appears in one
  frame, which is exactly what "leaving blind" means. There is no separate
  blind programmer to merge.
- Playback is **not** blind: running cues still drive the rig while blind, so
  blind editing during a show does not freeze it. The programmer is the only
  layer that goes dark.
- `Status` and the shell header carry a `BLIND` marker, and `Blind` is desk
  state (saved), so a crashed desk comes back still blind rather than
  unexpectedly live.

**Why one programmer, not two.** A second blind buffer raises merge questions
(does `Blind Off` overwrite? merge by attribute? re-track?) that a live desk
answers differently every time it is asked. Suppressing the contribution is the
smallest rule that produces the wanted behaviour and cannot corrupt a cue.

### Preview

```rust
pub struct Desk {
    pub preview: Option<Preview>,   // { sequence: u16, cue: f64 }
}
```

- `Preview Cue 14 [Sequence 2]` records the target; the engine can build a
  frame for it on demand:

  ```rust
  pub fn preview_frame(&self) -> Option<OutputFrame>;
  ```

  which renders `sequence.state_at(index)` through `Stage` with no programmer,
  no effects and **no masters** — the cue's own values are what is being
  inspected. Patch and types still apply, so the preview shows real DMX bytes.
- The frame is only ever displayed (`Sheet` shows a preview banner with the
  first non-zero channels; `Status` shows the cue and its value count). It never
  reaches `Shared` or a transport; the type system enforces this by returning an
  `OutputFrame` value that only the listing code consumes.
- `Preview Off` clears it. `Preview` with no cue previews the *next* cue of the
  running sequence, which is the common case.
- A preview survives edits (it is a reference to a cue number, resolved on
  each render) and disappears with a message if the cue is deleted.

**Why not "preview mode" that steals output.** A real desk can send a preview
to a second monitor or a desk screen. This engine has one output path, so the
honest scope is "show me the numbers", which is already useful for
troubleshooting a cue and costs no new output plumbing.

### Highlight

- `Desk.highlight: bool`. When on, `Stage` forces `Dimmer = 1.0` for every
  **selected** fixture, after the programmer/playback/home chain and before
  masters. Colour and position stay where they are, so highlight identifies a
  fixture without destroying the look it is in.
- It is an output aid, so it is suppressed while blind (a blind desk has
  nothing to look at) and while blackout is on (blackout wins; safety beats
  convenience).
- `Highlight` / `Highlight Off`; cleared automatically by `Clear` (selection
  gone) and by `Release`/`Blackout`.
- Highlight is a render-stage override, never a programmer write, so it cannot
  be stored in a cue by accident.

### Commands

| Command | Effect |
|---|---|
| `Blind` / `Blind Off` | suppress / restore the programmer's output |
| `Preview [Cue <n>] [Sequence <n>]` | inspect a cue's output |
| `Preview Off` | stop inspecting |
| `Highlight` / `Highlight Off` | full intensity on selected fixtures |

## Implementation plan

1. Add `Desk::{blind, preview, highlight}` + desk-file fields with defaults.
2. Change `Engine::render` to choose the programmer source; add
   `Engine::preview_frame`.
3. Add the highlight set to `Stage` (a `&[u16]` plus a bool) and apply it after
   the base chain; keep it out of `Stage::level`'s cache so nothing else sees it.
4. Add the parse arms, dispatch, `Status`/header markers, and the preview block
   in the `Sheet` listing.
5. Wire `Clear` and `Blackout` to clear highlight; wire cue deletion to clear a
   dangling preview.

## Tests

- **render** — with blind, the programmer is invisible and playback still
  renders; `Blind Off` shows it again; highlight raises only selected fixtures
  and only intensity; highlight + blackout is black; a preview frame is byte
  identical to a normal render of the same cue with no programmer.
- **engine** — a blind `At` then `Store Cue` captures the value; `Status` shows
  `blind`; preview of a missing cue errors naming the sequence and cue; the
  preview survives a programmer edit and disappears when the cue is deleted;
  highlight clears on `Clear`.
- **desk** — blind and highlight round-trip; preview is `None` after reload
  (a preview is a transient view, not a saved intent).
- **console/ui** — the header shows `BLIND`; the sheet shows the preview banner.

## Boundaries

- No second output device, no preview on the rig, no follow-spot or highlight
  colour options.
- No `Blind` for playback, no blind cue list editing grid (that is item 18's job
  to present; the engine already allows it).
- No highlight timeout (auto-clearing while an operator looks at the stage is
  worse than leaving it on).

## Risks

- **Blind is easy to leave on.** The header marker plus `Status` is the guard;
  the alternative (auto-expiring blind) surprises the operator mid-edit.
- **Highlight and blackout interact.** Blackout wins, documented, because a
  blackout that a highlight can undo is not a blackout.

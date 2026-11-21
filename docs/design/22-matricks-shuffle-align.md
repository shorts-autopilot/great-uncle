# 22 · MAtricks, Shuffle and Align

**Rank** 5 (follow-up) · **Order** 22 · **Size** L · **Show —, desk v1 (additive)** · **Depends on** 03 (desk state), 08 (16-bit attributes), 18 (selection listing)

## Problem

Multi-fixture editing is still one fixture at a time. A selection is an ordered
list of numbers, and `At 50` writes the same value to every fixture in it. There
is no way to divide ten fixtures into pairs, deal them into two interleaved
groups, fold the ends of a fan together, or spread a value from 0% at the first
fixture to 100% at the last. The whole `MAtricks` chapter and the `Align`
keyword are unbuilt.

That missing layer is where programming time goes. Fans, sidelight runs and
colour gradients are normally made by selecting subsets and retyping values.
Doing it by hand is slow, and the result cannot be reproduced from the command
stream later. The fix is a pure selection tool: no show-data field, no output
path change, and no hardware gate of its own.

## Behaviour when this is done

```
MAtricks                                  list the selection shape and cells
MAtricks XBlock 2                         merge consecutive fixtures into cells
MAtricks XGroup 2                         deal cells into two interleaved lanes
MAtricks XWings 2                         fold cells from the outside ends
MAtricks XWidth 3                         transpose wrapped rows into columns
MAtricks Transform Mirror                 mirror aligned values around the centre
MAtricks Transform Off                    stop mirroring
MAtricks Reset                             every MAtricks setting back to default
Shuffle                                   advance to the next deterministic order
Shuffle Seed 42                           choose a particular shuffled order
Shuffle Off                               restore the addressed order
Align Dimmer 0 Thru 100                   spread Dimmer across the cells
Align Pan 25 Thru 75 Transition Sinus     use a curve for this alignment
Align 0 Thru 100                          Dimmer is the default attribute
AlignTransition Slow                      set the default curve
Align                                     list the transition, mirror and cells
```

```text
> Fixture 1 Thru 6
Selected 1 Thru 6 (6 fixtures)

> MAtricks XBlock 2
MAtricks XBlock 2

> MAtricks
MAtricks  XBlock 2  XGroup 1  XWings 1  XWidth 0  Shuffle off  Transform Off
Selection 1 Thru 6 in 3 cells
   1  1 + 2
   2  3 + 4
   3  5 + 6

> Align 0 Thru 100
Aligned 1 Thru 6 Dimmer 0% Thru 100% across 3 cells

> MAtricks Reset
MAtricks reset

> MAtricks XWings 2
MAtricks XWings 2

> MAtricks
MAtricks  XBlock 1  XGroup 1  XWings 2  XWidth 0  Shuffle off  Transform Off
Selection 1 Thru 6 in 3 cells
   1  1 + 6
   2  2 + 5
   3  3 + 4
```

- **MAtricks is desk state, not show data.** It describes the current working
  selection, like the active page or a fader position. It is saved in the desk
  file with serde defaults, so an older desk loads at block 1, group 1, wings
  1, width 0, shuffle off and transform off. The show format is unchanged and
  `SHOW_VERSION` does not move.
- **The selection order is the operator's order.** A numeric range is sorted
  ascending by the parser; a group keeps its stored member order. MAtricks and
  Align never sort the selection again, because order is the whole point.
- **A cell is one value step.** Blocks merge consecutive fixtures into a cell,
  groups merge interleaved fixtures into one cell per lane, wings merge
  fixtures from opposite ends, and width merges columns of a wrapped grid.
  Every fixture in a cell receives the same aligned value.
- **The pipeline is block, group, wings, width.** Each stage works on the cells
  produced by the previous one, so the settings compose rather than replacing
  one another. For example, six fixtures with `XBlock 2` and `XGroup 2` become
  `1 + 2 + 5 + 6` and `3 + 4`; adding `XWings 2` folds those two cells into
  one. A width of zero or one means no wrapping; a width of at least two
  transposes cells into columns.
- **Shuffle changes the order before cells are built.** A seed produces the
  same permutation every time, using a local SplitMix64 stream and Fisher-
  Yates; no clock or operating-system random source is involved. Seed zero is
  off. Bare `Shuffle` advances the seed by one, cycling at 32767, so repeated
  commands are reproducible. `Shuffle Off` returns to the addressed order
  without changing the other MAtricks settings.
- **Align writes one interpolated value per cell.** `Align [<attribute>]
  <from> Thru <to>` uses the selection's Dimmer when no attribute is named.
  The values are clamped to 0..100, resolved through each fixture's own mode,
  and written into the programmer as normalised levels. A cue or palette
  stores them exactly like any other programmer edit.
- **Fine attributes get the full interpolation.** The programmer keeps an
  `f64`; rendering maps that through the profile's channel to coarse and fine
  bytes. Three moving fixtures aligned across `Pan 0 Thru 100` therefore put
  127/255 in the middle fixture's pan bytes, not an eight-bit-only 128.
- **A curve changes the spacing, never the endpoints.** `Linear` is equal
  spacing. `Sinus` has small gaps at the ends and large gaps in the centre.
  `Slow` starts with small gaps; `Fast` starts with large gaps. The curve is
  applied before the value is scaled between `from` and `to`.
- **Mirror folds values around the centre.** The first ceil(n/2) cells carry
  the range and the rest copy back from the centre. A centre cell stays at the
  end value; an even selection has a centre pair at the end value. Mirror is a
  value transform, not another reordering, so it works on top of blocks,
  groups, wings, width and shuffle.
- **One command is one programmer edit.** Align uses the ordinary
  `edit_programmer` path, so it is one undo step and appears in the journal as
  a programmer change. A failed validation (no selection, an attribute missing
  from one fixture, a filter blocking the feature) changes nothing.
- **The listings are diagnostic.** `MAtricks` prints the shape, selection and
  the first sixteen cells; `Align` prints the current transition, mirror state
  and cell count. Neither is an output change.

## Design

### Selection cells

`crates/engine/src/matricks.rs` owns the pure selection maths:

```rust
pub struct Matricks {
    pub block: usize,   // default 1
    pub group: usize,   // default 1
    pub wings: usize,   // default 1
    pub width: usize,   // default 0 (no wrapping)
    pub shuffle: u16,   // 0 is off; 1..=32767 is a seed
    pub mirror: bool,   // default false
}

pub enum MatricksProperty {
    Block, Group, Wings, Width, Shuffle,
}
```

`Matricks::cells(&[u16]) -> Vec<Vec<u16>>` is a pure function:

1. Deduplicate the selection while preserving its order.
2. If a shuffle seed is set, permute with Fisher-Yates over a seeded
   SplitMix64 stream.
3. Chunk consecutive fixtures into block cells.
4. Deal block cells round-robin into `group` lanes and merge each lane.
5. Split the cells into `wings` contiguous lanes, then merge the cells at the
   same depth, reading odd-numbered wings from their far end. For six
   fixtures and two wings this is `[1,6] [2,5] [3,4]`.
6. If `width >= 2`, wrap the cells into rows and merge each column, last short
   row included.
7. Return the cells in the order generated by those steps.

Values are not stored in the cells. That keeps MAtricks usable by other future
tools and makes `cells` cheap to test without an engine, show or clock.

### Align values

`aligned_levels(cell_count, from, to, transition, mirror) -> Vec<f64>` returns
the normalised value for each cell. It handles the empty and one-cell cases
explicitly: one cell takes `to`, because it is both ends of the range.

The transition is a small enum with a pure `apply(0..=1) -> f64` method:

| Name | Shaping |
|---|---|
| `Linear` | `t` |
| `Sinus` | `0.5 - 0.5 * cos(pi * t)` |
| `Slow` | `t * t` |
| `Fast` | `1 - (1 - t) * (1 - t)` |

`AlignTransition` has a serde default (`Linear`) and a label/parse pair, so it
can be stored in the desk and set by name or by the manual's 0..=3 index in a
later command surface.

### Desk state

`Desk` gains two fields:

```rust
pub matricks: Matricks,
pub align_transition: AlignTransition,
```

Both are saved in `DeskFile` with `#[serde(default)]`. They are session working
state, not show data:

- `MAtricks` settings describe how the current selection is divided; they
  should survive an autosave or a desk reload, but they do not belong in a show
  that travels to another console.
- `align_transition` is the default curve for the next Align, like a fader
  position.
- Neither field needs a desk version bump under the additive rules in
  `ROADMAP.md`; older files read at the defaults. `Desk::sanitise` repairs
  impossible hand-edited values (zero block/group/wings, values above 4096,
  seeds above 32767) and reports each repair as a warning.
- `MAtricks Reset` resets the six shape fields; it deliberately does not reset
  the Align transition, which is a separate command setting.

### Commands and listings

| Command | Effect |
|---|---|
| `MAtricks` | list the shape, selection and cells |
| `MAtricks XBlock <n>` | merge `n` consecutive fixtures per cell; 0 is off |
| `MAtricks XGroup <n>` | deal into `n` interleaved lanes; 0 is off |
| `MAtricks XWings <n>` | fold `n` wings from their ends; 0 is off |
| `MAtricks XWidth <n>` | wrap at `n` cells per row; 0 or 1 is no wrap |
| `MAtricks Transform Mirror\|Off` | mirror aligned values |
| `MAtricks Reset` | reset the shape |
| `Shuffle [Seed <n>] [Off]` | choose or clear a deterministic order |
| `Align [<attribute>] <from> Thru <to> [Transition <name>]` | write the range |
| `AlignTransition [<name>]` | list or set the default curve |
| `Align` | list the transition, mirror and cell count |

`MAtricks` accepts the manual's `XBlock`, `XGroup`, `XWings`, `XWidth` and
`XShuffle` names as well as the shorter spellings. The X prefix is retained in
the listing so a future Y/Z implementation has an obvious home. `Transform`
also accepts `Off` and `None`.

`Align` is a one-shot range command rather than an encoder mode. It resolves
the attribute against every selected fixture before touching the programmer;
if any fixture lacks the channel, the command names that fixture and the mode's
available attributes. It also checks the active filter and world through the
same helpers as `At` and `Color`. Values are written only after every check
passes.

### Format

The show format does not change. The desk file is additive:

```json
"matricks": {
  "block": 2,
  "group": 1,
  "wings": 2,
  "width": 0,
  "shuffle": 42,
  "mirror": true
},
"align_transition": "sinus"
```

A desk written before this item has neither field and loads at the defaults. A
hand-edited value that cannot mean anything is repaired with a warning rather
than making the whole desk unusable.

## Implementation plan

1. `engine/matricks.rs`: `Matricks`, `MatricksProperty`, `AlignTransition`,
   `cells`, `aligned_levels` and their pure unit tests.
2. `desk.rs`: the two desk fields, defaults, additive file fields, sanitise
   repairs and round-trip tests.
3. `command.rs`: the command variants, parser functions, reserved words,
   `mutates` classification, help text and parser tests.
4. `engine.rs`: dispatch, listings, shuffle seeds, transition setting and the
   Align programmer edit, including a 16-bit pan test.
5. This design, `ROADMAP.md`, `PHASE5.md`, `CHANGELOG.md`, and regenerated
   `COMPLETENESS.md`/matrix rows.

## Tests

- **matricks** — default cells are one per fixture; blocks merge consecutive
  fixtures; groups interleave and merge into lanes; wings pair outside ends;
  width transposes wrapped rows; all settings compose in the documented order;
  a selection with duplicate numbers is deduplicated without reordering.
- **shuffle** — the same seed produces the same permutation; the selection's
  multiset is preserved; a different seed produces a different order; seed zero
  never shuffles; a bare `Shuffle` advances one and wraps at 32767.
- **align values** — linear, sinus, slow and fast curves; a one-cell selection
  takes the end value; mirror on even and odd cell counts; names and numeric
  indices round-trip.
- **parser** — every command in the table parses; malformed property names,
  oversized block/width values, oversized seeds, a missing `Thru`, an
  out-of-range level and an unknown transition name all fail with a helpful
  message; listing commands are non-mutating.
- **engine** — blocks, groups, wings and mirror reach the programmer; shuffle
  is repeatable and returns to the addressed order; transition setting changes
  the default; MAtricks listing names shape and cells; Reset restores every
  field; Align rejects an attribute missing from one fixture.
- **16-bit** — a three-fixture moving selection aligned across `Pan 0 Thru 100`
  writes 0, 0.5 and 1.0 to the programmer and renders the middle fixture's pan
  as coarse 127/fine 255 rather than an eight-bit-only byte.
- **desk** — the new fields round-trip; missing fields load at defaults;
  impossible block/group/shuffle values are repaired with warnings.
- **standard gate** — `cargo test --workspace`, workspace and GUI clippy,
  `cargo fmt --check`, and `python3 tools/spec_status.py --check`.

## Boundaries

- **One axis.** This item implements the X-axis selection maths on a flat
  selection. Y/Z axis counts, axis swap/invert, the real Selection Grid window
  and 3D fixture positions are deliberately left to a later grid design.
- **No MAtricks pool.** Settings are temporary desk state, not stored,
  numbered MAtricks objects attached to groups or presets.
- **No per-axis shuffle modes.** The manual's Auto, Linked and Unlinked modes
  and separate X/Y/Z seeds wait for the real grid; one seed over the whole
  selection is the reproducible core.
- **No encoder align modes.** The manual's `<`, `>`, `><` and `<>` modes
  describe how an encoder increments a value. This command interface has no
  encoder wheel; it writes an explicit range with a curve and an optional
  mirror instead. `AlignTransition` is the part of that model that survives a
  command line.
- **No calculator shorthand.** `At 75 Thru 10 Thru 75` stays unsupported;
  `Align` is the explicit, discoverable form.
- **No pixel mapping, XYZ or MVR position use.** The cells are selection
  order, not physical geometry.
- **No GUI controls.** The terminal listing and the command line are the whole
  surface in this item; a grid window is GUI depth.
- **No hardware claim.** This is engine and desk work. It does not move any
  row to `verified`; the physical output/display proof remains the release
  gate.

## Risks

- **Composition is simpler than the source grid.** Blocks, groups, wings and
  width all merge cells, so turning on several settings can collapse a
  selection further than a full 2D/3D grid would. The first implementation is
  deliberately one-dimensional and the listing shows the resulting cells, so
  an unexpected shape is visible before a value is written.
- **Group and block are easy to confuse.** Both reduce the number of value
  steps. Blocks are consecutive; groups are interleaved. The examples in this
  document and the cell listing use fixture numbers so the difference is
  testable.
- **Shuffle is not a random performance tool.** It is a deterministic
  permutation for selection editing. A repeated `Shuffle` advances predictably
  rather than drawing fresh entropy, which is the property that makes a show
  reproducible.
- **Align is not a cue-level fade.** It writes a static programmer value; a
  fade between two aligned looks still needs a cue or a phaser. Storing an
  aligned look captures the values, not the MAtricks recipe.
- **Mirror can surprise with an even selection.** The centre pair both reaches
  the end value. That is the documented symmetry rule, and the cell count in
  the listing makes it visible.
- **A desk reload keeps the shape.** An operator who expected a fresh
  selection tool may find an old block/group still active. `MAtricks` shows it
  and `MAtricks Reset` clears it.
- **Hardware is still the release gate.** None of these tests exercise a real
  DMX adapter or a display. The physical path remains first in the follow-up
  ranking and no row is verified until it is proven there.

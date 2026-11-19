# 11 · Patch tools and reports

**Rank** 11 · **Order** 11 · **Size** M · **Show** — · **Depends on** 03

## Problem

Patching is one fixture at a time (`Patch Fixture 1 At 1.001 Type …`). A rig of
24 identical washes is 24 commands with 24 addresses typed by hand, and the only
overlap check is the one `PatchRange::conflicts` performs on load. There is also
no way to hand the patch to a stage hand, no way to fill a show in from a
spreadsheet, and no report an operator can print before a fit-up.

## Behaviour when this is done

```
Clone Fixture 1 At 1.101 Count 8        eight more washes at 1.101 … 1.401
Patch Check                             overlaps, gaps, footprint overflow
Patch Import "rig.csv"                  patch from a spreadsheet
Report Patch "patch.csv"                one row per fixture, same columns
Report Cues 1 "cues.csv"                the cue list as data
Report Show "show.md"                   the whole desk as a document
```

```text
Patch Check
  24 fixtures, 96 slots, universes 1
  no overlaps
  universe 1: 96 of 512 slots used, largest gap 413 slots at 1.097
```

## Design

### Clone

```
Clone Fixture <n> At <universe>.<address> [Count <k>] [Step <slots>]
```

- Copies type, mode and home from the source fixture, names them
  `<name> 2…k`, numbers them from the next free fixture number, and places them
  at `address + i * step` where `step` defaults to the source mode's footprint.
- Atomic: every new range is checked for overlap against the whole patch
  (including the clones among themselves) *before* anything is inserted, so a
  failure leaves the patch untouched. This is the same all-or-nothing rule
  `mutate` already enforces, applied to a multi-fixture edit.
- `Clone Fixture 1 Thru 4` is not in scope: cloning a pattern from one fixture
  is the common case, and a multi-source clone needs a naming rule nobody
  agrees on.

### Patch check

`Patch Check` is a read-only listing built from three existing pieces plus one
new one:

- overlaps: `patch::conflicts` (exists);
- footprint overflow: `PatchRange::fits(universe.size)` per fixture (exists);
- gaps: walk the sorted ranges per universe and report runs of unpatched slots
  larger than 8 (a new, ~20-line helper with its own tests);
- type/mode resolution: list fixtures whose type or mode is missing (validation
  rejects these on load, so this is for a hand-edited file).

Output is a short summary plus one line per problem, which makes it usable in
CI as `openlight --check` (item 16) as well as by eye.

### CSV

One column set, used by both import and export so a round-trip is lossless:

```csv
number,name,type,mode,universe,address
1,Demo Wash,OpenLight Demo Wash,4ch,1,1
```

- `Patch Import "rig.csv"`: replaces the patch. The header row is required and
  matched case-insensitively; unknown columns are ignored with a warning; every
  row is validated before the patch is applied. `type` must already be imported
  (the error names the missing type and suggests `Import Gdtf`). Duplicate
  numbers and overlapping ranges are reported with row numbers.
- `Report Patch "patch.csv"` writes the same columns, in fixture-number order.
- A quoted field containing a comma is the only CSV feature supported; the
  parser is ~60 lines and refuses anything else rather than silently
  mis-parsing.

### Reports

`Report <kind> [target] <file>` writes text that the engine already knows how to
produce:

| Kind | Source of truth |
|---|---|
| `Patch` | the clone/check table above (CSV) |
| `Cues [<sequence>]` | `Sequence` listing lines (CSV for data, text otherwise) |
| `Palettes` | `Palette` listing |
| `Status` | `Status` output |
| `Show` | every listing in order, as one Markdown document |

The point of routing reports through the existing listing functions is that a
report cannot disagree with the screen. `Report Show` is literally the shell's
five windows concatenated.

All reports are read-only: no journal entry, no undo step, no revision bump.

## Implementation plan

1. `patch::gaps` + `Patch Check` dispatch and listing.
2. `Clone Fixture` parse/dispatch, using the existing range validation, with the
   atomicity test.
3. `crates/engine/src/csv.rs`: a minimal reader/writer for the one column set,
   with quoting tests; `Patch Import` and `Report Patch`.
4. `Report` parse/dispatch; reuse listing builders by extracting the functions
   the shell already calls into `Engine` methods where they are still inline.
5. `--check` in the console (wired fully in item 16).

## Tests

- **patch** — gaps are reported per universe with correct run lengths; a range
  ending exactly at the universe size is not a gap; a fully patched universe has
  none.
- **engine** — clone of 8 occupies 8 × footprint with no overlap; a clone that
  would overlap is refused and leaves the patch unchanged (compare the whole
  show before/after); clone names are numbered and unique; `Patch Check` finds
  a planted overlap and an overflow.
- **csv** — round-trip preserves every column; quoted commas; a missing header
  is an error naming the expected columns; a bad address reports the row
  number; an unknown type is refused with a hint; import does not partially
  apply.
- **engine** — `Report Show` output equals the concatenation of the listings;
  reports write nothing to the journal and bump no revision.

## Boundaries

- No CSV of cue *values* (only the cue list as data); moving programming between
  shows is not this item.
- No XLSX, no delimiters other than comma, no encodings other than UTF-8.
- No automatic address allocation by range ("next free"), because the operator
  always has an opinion; `Clone` with an explicit address is enough.
- No PDF/HTML rendering.

## Risks

- **CSV as an interchange format invites hand editing.** Mitigated by
  validating everything before applying and reporting row numbers; the show file
  remains the only format the engine trusts.
- **Report drift.** The whole design is "call the listing"; if a listing is
  inlined in the shell instead of an `Engine` method, the report cannot use it.
  Step 4 is therefore a prerequisite, not polish.

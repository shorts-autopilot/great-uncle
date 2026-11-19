# 13 · MVR import and a fixture builder

**Rank** 13 · **Order** 15 · **Size** L · **No format change** · **Depends on** 08 (full profiles), 11 (patch import)

## Problem

Two ends of the same gap: getting a rig into the desk, and fixing a profile once
it is there.

- **MVR** (ANSI E1.77 / DIN SPEC 15801) is the standard exchange format for a
  rig: a zip with a scene description, the GDTF profiles and the fixture
  positions. Today every fixture is patched by hand from a GDTF the user has to
  find first.
- **Profiles are read-only.** If a supplied GDTF has a wrong channel offset, a
  missing mode or a fine channel the importer cannot pair, the user's only
  option is to edit XML inside a zip. There is no way to create a profile from a
  channel list at all.

## Behaviour when this is done

```
Import Mvr "rig.mvr"
  24 fixtures, 3 types imported, 24 GDTF archives
  warnings: 2 fixtures skipped (no GDTF in archive)
```

```
Type New "Generic 4ch"
Type "Generic 4ch" Mode "4ch"
Type "Generic 4ch" Channel 1 Dimmer "Dimmer"
Type "Generic 4ch" Channel 2 ColorAdd_R "Red"
Export Type "Generic 4ch" "generic-4ch.olft"
Copy Type "OpenLight Demo Wash" As "Demo Wash (wide)"
```

## Design

### MVR import

`Show::import_mvr(path) -> ImportReport` in `crates/engine/src/mvr.rs`, reusing
the `zip` dependency already used by `gdtf.rs`.

1. Read `GeneralSceneDescription.xml` (version 1.x; the root element and
   `<Scene>` wrapper are accepted in either namespace form seen in the wild).
2. Walk `Layers → ChildList → Fixture` (`name`, `GDTFSpec`, `GDTFMode`,
   `Address`, `Matrix`). Ignore `SceneObject`, `GroupObject`, `Truss` and
   geometry: this item imports patch, not geometry.
3. For each distinct `GDTFSpec` used, extract `*.gdtf` from the archive into a
   temp dir and run the existing GDTF importer. A missing profile is a
   **warning per fixture**, not an error: a rig with one unknown type should
   still import the other 23.
4. Patch fixtures at their MVR address. `Address` is `"universe.address"`; the
   universe must fit `MAX_UNIVERSE_NUMBER` and the address inside the universe,
   else the fixture is skipped with a warning that names it.
5. Positions (`Matrix`) are **dropped with a counted warning** (`24 positions
   ignored: no 3D model`). Storing a position with nothing that reads it would
   be dead data in the show file.
6. The whole import is one `mutate`: types, universes and fixtures arrive
   together or not at all. Fixture names come from MVR, numbers are assigned
   from the next free number in import order, and a name collision gets a
   numeric suffix.
7. `--mvr <file>` at startup runs it before any `--script` line, so the existing
   demo scripts can be pointed at a real rig.

**Ambiguity rule:** MVR gives no stable fixture numbers (only names). Assigning
in document order is deterministic and documented; a user who cares about
numbers patches afterward (item 11's CSV).

### Fixture builder

A profile is data, so it is edited as data — no GUI (item 18) and no XML
editing.

```json
// generic-4ch.olft
{ "format": "openlight-fixture-type", "version": 1,
  "manufacturer": "Generic", "name": "4ch",
  "attributes": [{ "name": "Dimmer", "pretty": "Dimmer", "feature": "Dimmer" }],
  "modes": [{ "name": "4ch", "channels": [
      { "offset": 1, "attribute": "Dimmer", "default_raw": 0,
        "functions": [{ "name": "Dimmer", "dmx_from": 0, "dmx_to": 255,
                        "physical_from": 0, "physical_to": 100 }] }]}],
  "home": { "ColorAdd_R": 1.0 } }
```

- `crates/engine/src/builder.rs`: `FixtureTypeFile`, `from_file`, `to_file`,
  validation (offsets unique and non-zero, attributes referenced by every
  channel exist, functions inside 0..=255 and ordered, mode names unique,
  footprint ≤ 512). Errors name the field and the index.
- `Import Type <file>` adds the type to the show (replacing a type of the same
  key with a warning that says how many fixtures use it).
- `Export Type "<key>" <file>` writes the current type, so a GDTF can be fixed,
  exported, edited and re-imported.
- `Copy Type "<key>" As "<new key>"` duplicates with a new name and mode names
  unchanged; the fastest fix for "I need two modes of the same fixture".
- The keyboard commands (`Type New …`, `Type … Channel …`) are a thin layer
  over the same struct for small edits; they are listed here because they share
  the builder's validation, not because they are a second format. `Type` is the
  command noun throughout (`Export Type`, `Copy Type`, `Import Type`), matching
  the single-word nouns every other command uses. One command family, one
  validator.

### Why both

MVR gets a whole rig in; the builder fixes the profile that a rig came with.
Importing MVR without a builder means a bad profile blocks the import with no
recourse; a builder without MVR means the user still patches by hand. 08 is a
prerequisite because an MVR rig almost always contains 16-bit movers, and
importing them as 8-bit would silently degrade the rig.

## Implementation plan

1. `mvr.rs`: archive layout, scene walk, per-fixture extraction and the
   `ImportReport` with counted warnings; tests build a small `.mvr` in-memory
   with the `zip` crate.
2. Reuse `gdtf::parse_description` on extracted bytes (add a
   `parse_description_bytes` entry point) so no temp files are required for
   profiles without external references.
3. `builder.rs`: file type, validation, import/export; `Copy Type`.
4. The `Type …` command family backed by the builder struct.
5. `--mvr` wiring and README examples.

## Tests

- **mvr** — a well-formed archive imports types, universes and addresses; two
  fixtures of one type import one profile; a missing GDTF skips that fixture and
  keeps the rest; an out-of-range address is skipped with its name; a broken zip
  is an error that names the entry; positions produce one counted warning.
- **mvr** — importing twice replaces rather than duplicating (same fixture
  numbers if the show was empty; otherwise new numbers, documented).
- **builder** — round-trip of a type through `.olft`; duplicate offsets, a
  channel for an undeclared attribute, and a function with `dmx_from > dmx_to`
  are each rejected naming the index; a mode footprint over 512 is rejected.
- **engine** — `Import Type` makes the type patchable; re-importing a type used
  by a fixture warns with the count and keeps the patch valid; `Copy Type`
  produces a distinct key that patches independently.
- **show** — a show containing a builder-created type survives save/load and
  renders the same bytes.

## Boundaries

- **No geometry, no 3D preview, no layers as groups.** MVR is used as a patch
  and profile carrier.
- **No MVR export.** Writing a conforming scene description is a second project;
  the GDTF/`.olft` round trip covers the profile half.
- No `.gdtf` *writing* (only reading), no wheel/gobo image handling.
- No profile library or downloads.

## Risks

- **MVR dialect variance.** Real archives differ in namespaces and optional
  elements. The parser is written to tolerate unknown children and to warn per
  fixture rather than fail the import; the test fixture covers the documented
  shape and one variant without `<Scene>`.
- **Profile replacement is destructive.** Replacing a type changes how every
  fixture of that type renders. The warning names the fixture count and the item
  keeps the *show* valid rather than the *look* identical; undo (03) covers it.
- **Builder validation drift.** One validator shared by file import, command
  edits and GDTF conversion means a profile cannot be valid through one door and
  invalid through another; the tests exercise all three doors against the same
  cases.

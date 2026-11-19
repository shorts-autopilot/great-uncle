# 08 · 16-bit attributes and channel sets

**Rank** 8 · **Order** 10 · **Size** M · **Show** v10 · **Depends on** 07 (home inverse), 03

## Problem

Two gaps in how a profile becomes an attribute.

**16-bit.** A profile that gives pan or tilt a fine channel is imported as
8-bit and the importer warns: `16-bit function 'Pan' treated as 8-bit`. The fine
byte is dropped, so a moving head can only be positioned in 256 steps and a slow
pan glides in visible stair-steps. The profile's fine channel still occupies a
slot, so the mode's footprint is right but one of its slots never changes.

**Channel sets.** A `ChannelFunction` is already a named band — `Gobo 1` at
0..15, `Rotate CW` at 128..191 — but nothing can address it. The operator has to
guess a percentage that lands inside the band, and a wheel with a 4% window is
guesswork. `raw_from_level` picks *the first function covering the physical
target*, which means sets are reachable only by luck.

## Behaviour when this is done

```
Fixture 1 Pan 42.5              stored at full precision, written as two bytes
Fixture 1 Gobo 2                second set of the wheel, by index
Fixture 1 Gobo "Rotate CW"      by name
Fixture 1 Gobo                  list the sets and the current one
```

```text
Fixture 1 Gobo (16-bit: no)
  1  Gobo 1     10..19%   (DMX 0..15)
  2  Gobo 2     20..39%   (DMX 16..47)
  3  Rotate CW  40..59%   (DMX 48..95)
  current: Rotate CW (DMX 71)
```

## Design

### 16-bit channels (show v10)

```rust
pub struct Channel {
    pub offset: u16,              // coarse slot, 1-based
    #[serde(default)]
    pub fine_offset: Option<u16>, // slot of the low byte, 1-based
    pub attribute: String,
    pub default_raw: u8,
    pub functions: Vec<ChannelFunction>,
}
```

- `Mode::footprint()` takes `max(offset, fine_offset)` so a 16-bit channel's
  fine slot is inside the patch range.
- `Channel::raw16_from_level(level) -> u16` reuses the band selection of
  `raw_from_level`, then maps the byte range through `×257`
  (`0..=255` → `0..=65535`) and interpolates: a value at 50% is 32767, split
  `0x7F 0xFF`. `raw_from_level` becomes `raw16_from_level >> 8`, so there is one
  interpolation implementation, not two.
- `render` writes two slots for a fine offset: `slots[coarse] = hi`,
  `slots[fine] = lo`, each bounds-checked.
- `default_raw` seeds the coarse byte; the fine byte starts at 0, which is the
  profile's own default for a 16-bit attribute.

**Import.** `gdtf.rs` currently computes `wide = from.0 != 0 || to.0 != 0` and
warns. Instead, `from.0` is the fine channel's offset relative to the coarse
channel (`DMXFrom="1/0"` means "fine slot is one past this channel"). When it is
non-zero, set `fine_offset = Some(channel.offset + from.0)` and drop the warning.
`to.0` must equal `from.0`; a profile that disagrees gets a warning and is
imported as 8-bit, because there is no sane way to guess which byte moves.

### Channel sets

No new stored data: a set **is** a `ChannelFunction`. That is why this item is
cheap — the profile already contains everything.

```rust
impl Channel {
    pub fn sets(&self) -> impl Iterator<Item = (usize, &ChannelFunction)>;
    pub fn level_for_set(&self, selector: SetSelector) -> Option<f64>;
}

pub enum SetSelector<'a> { Index(usize), Name(&'a str) }
```

`level_for_set` returns the midpoint of the function's physical range, clamped
to `0..=1`. Going through the physical range (rather than the DMX byte) keeps
the existing "programming is physical" rule: the stored value is still a normal
level, so it fades, stores in palettes and cues, and prints as a percentage. For
a discontinuous wheel the physical ranges are distinct by construction (GDTF
bands do not overlap), so the midpoint round-trips to a raw byte inside the
right band — asserted by a test for every set of the demo profile.

Name matching is case-insensitive and exact (no prefix matching: `Gobo 1` must
not resolve `Gobo 10`). Duplicate names in one channel are allowed by GDTF but
ambiguous to address; the importer already disambiguates nothing, so
`level_for_set` returns the first and `Fixture 1 Gobo` lists the duplicate so it
is visible.

### Commands

| Command | Effect |
|---|---|
| `Fixture 1 Gobo 2` | select set 2 (1-based, in DMX order) |
| `Fixture 1 Gobo "Rotate CW"` | select by name |
| `Fixture 1 Gobo` | list the sets, current selection and DMX range |
| `Group 1 Gobo "Open"` | idem through a group |

The grammar is `Fixture <n> <attribute> <set>` — the attribute is already a
word, so this only adds a third word where `At` used to be required. `At` stays
for numbers: `Fixture 1 At Gobo 40` is unchanged and still means "40% of the
attribute's physical range".

Ambiguity rule: a quoted token is always a set name; a bare integer after an
attribute is a set index; a percentage needs `At`. There is no case where a set
index and a level are both plausible, because a level without `At` is not
grammar today.

### Listing

`Sheet` keeps showing the current value; the per-attribute listing
(`Fixture 1 Gobo`) is where sets are shown, because printing 30 sets per
attribute in the sheet would bury the rig. The listing prints the index, name,
physical band, DMX band and a `current:` line.

## Implementation plan

1. Add `fine_offset`, `Mode::footprint` fix, `raw16_from_level` (with
   `raw_from_level` as `>> 8`), and `level_from_raw` extended to the 16-bit
   range; port the existing fixture-type tests and add split tests.
2. Teach `gdtf.rs` to pair fine channels, drop the 16-bit warning, and keep the
   warning for inconsistent fine offsets; add the two new import tests.
3. Render both bytes, with a bounds check and a `debug_assert` that the fine
   slot is not another channel's coarse slot (validation rejects overlap anyway).
4. Add `sets()`/`level_for_set`, the `SetSelector` parse arm, dispatch through
   `set_level`, and the listing.
5. Validate: a mode whose channels share a slot (including fine slots) is
   rejected at load, not at patch time.
6. Bump to v10 with a v9→v10 upgrade (`fine_offset: None`).

## Tests

- **fixture_type** — 16-bit at 0, 50, 100% gives `(0,0)`, `(127,255)`,
  `(255,255)`; `raw_from_level` equals the high byte for every level of a
  16-bit channel; band selection is shared (a two-band 16-bit channel picks the
  same band as its 8-bit form).
- **gdtf** — `DMXFrom="1/0" DMXTo="1/255"` at offset 3 yields
  `fine_offset = Some(4)` and no warning; mismatched `to.0` warns and imports
  8-bit; a mode's footprint covers the fine slot.
- **render** — both slots are written, adjacent fixtures do not bleed, a
  fine slot beyond 512 is skipped without panic.
- **engine** — `Fixture 1 Gobo 2` stores the second set's midpoint;
  `Fixture 1 Gobo "rotate cw"` resolves case-insensitively; `Fixture 1 Gobo 99`
  errors naming the set count; `Fixture 1 Gobo` lists with the current one
  marked; a group command applies to every member that has the attribute and
  reports the ones that do not.
- **show** — v9 upgrade imports with `fine_offset: None`; two channels sharing a
  slot are rejected with both attributes named.

## Boundaries

- No 24-bit or 32-bit attributes (no profile in scope needs them).
- No sub-attribute timing for the fine byte (item 02 already treats the
  attribute as one lane; the two bytes move together).
- No set editing or set palettes (a named set is already addressable; storing
  "Gobo 3" as a palette happens through the normal palette path).

## Risks

- **Profile disagreement.** GDTF archives in the wild encode fine channels
  inconsistently. The design keeps a warning for the inconsistent case rather
  than guessing, and the manual's fixture builder (item 13) gives a way to fix a
  profile by hand.
- **Physical midpoint is not always the intended DMX value.** A band 40..59%
  whose DMX is 48..255 (sparse) lands at 71% of the band, not at its start. The
  listing prints the resulting DMX so the operator can see exactly where the
  wheel lands.

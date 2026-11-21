# 19 · The colour model

**Rank** 2 (follow-up) · **Order** 19 · **Size** M · **No format change** · **Depends on** 07 (home), 08 (channel sets)

## Problem

The desk speaks one colour language: additive RGB levels. `ColorAdd_R/G/B` is
the only system the sheet swatch understands, and every colour is entered as
three separate `At` commands. A profile that uses subtractive CMY
(`ColorSub_C/M/Y`) must be programmed inverted by hand. HSB is arithmetic in the
operator's head. A colour wheel is addressable only by set index or exact set
name (item 08), never by the colour it produces. A named colour or a hex value —
the two things every other tool exchanges — cannot be typed at all.

The cost is not cosmetic. A colour cue on a CMY rig is written with the wrong
numbers unless the operator inverts them, a wheel cue needs the DMX chart, and
the GUI shows no swatch at all on a CMY fixture because it only reads
`ColorAdd_*`.

## Behaviour when this is done

```
Color Red                      the selection to sRGB #FF0000
Fixture 1 Color "#FF8000"      a hex colour
Group 2 Color Rgb 255 128 0    explicit additive components
Color Cmy 0 60 100             subtractive percentages
Color Hsb 30 100 50            hue, saturation, brightness
Color 3200K                    a blackbody white
Color                          list each fixture's system, values and hex
```

```text
> Color Red
Fixture 1 color Red (#FF0000)

> Color
Fixture 1  additive  R 100% G 0% B 0%  #FF0000
Fixture 2  subtractive  C 0% M 100% Y 100%  #FF0000
Fixture 3  wheel  set 2 "Red"  #FF0000
```

- One colour intent becomes what the fixture can actually do: RGB levels,
  inverted CMY levels, or the nearest named set on a colour wheel.
- An additive profile with `ColorAdd_W` gets the white a colour contains:
  `W = min(r, g, b)` and the primaries are reduced by it, the standard RGBW
  extraction. A saturated colour writes `W 0`, so a previous white does not
  contaminate it.
- `Color` with no argument is a query. It lists the rendered colour of every
  selected fixture and never mutates the programmer.
- On a colour wheel an exact name match wins; otherwise the nearest set whose
  name resolves to a colour is chosen. Index and quoted-name addressing from
  item 08 is unchanged.
- `Sheet`'s swatch and the GUI's follow the mode's own system (additive with
  white, subtractive, or the current wheel set), so the swatch shows the rig,
  not an assumption.
- Palettes and cues are untouched: a palette captures the levels the colour
  command wrote, per fixture type, exactly as before. No show or desk field is
  added, so the show stays v12 and the desk stays v1.

## Design

### Colour maths (new `crates/engine/src/color.rs`)

```rust
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

pub enum ColorSpec {
    Named(String),
    Hex(String),
    Rgb(u8, u8, u8),
    Cmy(f64, f64, f64),
    Hsb(f64, f64, f64),
    Kelvin(u16),
}
```

- `Rgb` is sRGB-encoded `0..=1`, the space DMX RGB levels already mean on a
  desk and the space the GUI swatch displays. There is one space for the whole
  desk; a colour-managed pipeline is out of scope.
- Conversions are the plain ones, deliberately: `C = 1 - R`, the standard HSB
  formula, and the Planckian-locus approximation for Kelvin, normalised so the
  brightest component is 1 (a white point must not dim the fixture). Kelvin is
  accepted from 1000K to 40000K.
- The named table is small and fixed: 17 names from `Black`, `Grey` and
  `White` through the six primaries and secondaries to `Warm White`, `Cool
  White` and `UV`. `named()` is case-insensitive; `Gray`, `Open`, `No Colour`,
  `No Color`, `CTO` and `CTB` are aliases. A name with spaces is quoted (`Color
  "Warm White"`), or written bare and joined from the remaining words (`Color
  Warm White`).
- Hex is exactly `#RRGGBB` (six digits, `#` required). Short hex and named CSS
  colours are not guessed.
- A wheel set's name is resolved through the same table, plus `CTO`/`CTB`
  prefixes and a trailing number (`Red 2` → `Red`). Unnamed or unrecognised
  sets remain addressable through item 08, not through colour.

### System detection and assignment

```
additive     ColorAdd_R + ColorAdd_G + ColorAdd_B    (ColorAdd_W extracted)
subtractive  ColorSub_C + ColorSub_M + ColorSub_Y
wheel        the first ColorWheel* channel with functions, in DMX order
```

- Additive beats wheel when a mode has both: a continuous system can hit the
  requested colour exactly, a wheel cannot.
- A partial system (say `ColorAdd_R` and `ColorAdd_G` with no blue) is
  **unsupported**, not guessed. The error names the fixture and the attributes
  it does have.
- `ColorSub_*`/`ColorAdd_*` names other than the six primaries (UV, amber,
  lime) are not written. `ColorAdd_UV` is not part of visible colour and stays
  where the operator put it.
- Wheel selection:
  1. an exact set-name match wins, even if another set is nearer;
  2. otherwise the named set with the smallest squared sRGB distance;
  3. a tie goes to the lowest set index, so the result is deterministic.
  The chosen set's level is the midpoint from `Channel::level_for_set`, so it
  fades, stores and prints like any other physical value. If no set name
  resolves to a colour, the command fails and lists the sets.

### Commands

| Command | Effect |
|---|---|
| `Color <name>` | set the selection to a named colour |
| `Color "#RRGGBB"` | set the selection to a hex colour |
| `Color Rgb 255 128 0` | explicit additive bytes |
| `Color Cmy 0 60 100` | explicit subtractive percentages |
| `Color Hsb 30 100 50` | hue 0..360, saturation and brightness 0..100 |
| `Color 3200K` | blackbody white |
| `Fixture 1 Color Red` | one fixture |
| `Group 1 Color Red` | every group member |
| `Color` | list the rendered colour of the selection |

`Color` becomes a reserved word. No GDTF attribute is named bare `Color` —
they are `ColorAdd_*`, `ColorSub_*` and `ColorWheel*` — so the only thing
shadowed is a hand-built profile that names a channel exactly `Color`; the
design accepts that because the colour command is more valuable than the
ambiguity, and the profile builder can name it `ColorAdd_*` instead.

The grammar follows the existing `Verb Noun` and `Noun Verb` shapes: bare
`Color` acts on the selection like `Home`, and `Fixture 1 Color …` is the same
shape as `Fixture 1 Home`. Values are validated in the parser, so a bad hex,
an out-of-range component or an unknown name never reaches the engine.

### Rendering and listings

The renderer does not change. Colour is still levels on channels, so 16-bit
attributes, home, effects, timing, MSO/MIB and palettes all keep working; the
colour model only decides which attributes to write.

`Stage` remains the single source of rendered values. `sheet_rows()` computes
the swatch with `color::swatch(mode, level)` from the same rendered levels it
prints in the cells, and `Color`'s listing uses the same function. The sheet
and the listing therefore cannot disagree, and the GUI inherits the fix through
`SheetRow.colour` with no GUI change.

`Color`'s listing prints the system, the component values (including `W` when
the mode has it), and the resulting hex:

```
Fixture 1  additive  R 100% G 0% B 0%  #FF0000
Fixture 2  subtractive  C 0% M 100% Y 100%  #FF0000
Fixture 3  wheel  set 2 "Red"  #FF0000
Fixture 4  unsupported: no additive (ColorAdd_R/G/B), subtractive
           (ColorSub_C/M/Y) or ColorWheel channel (attributes: Dimmer)
```

### No format change

Nothing is persisted. Palettes are keyed by fixture type, so an RGB palette
still cannot land on a CMY profile; within one type every fixture shares the
same system. A colour command writes ordinary programmer values, which the
existing journal, undo, autosave and cue capture already understand.

## Implementation plan

1. Add `color.rs`: `Rgb`, `ColorSpec`, named table and aliases, hex, HSB, CMY,
   Kelvin, system detection, assignment plans, wheel choice, swatch and
   description. Unit-test the maths and every system.
2. Add synthetic CMY and colour-wheel fixture types to `test_support`, so
   engine tests exercise real modes rather than fabricated levels.
3. Add `Command::SetColor` and the parser arms for the selection, fixture and
   group forms; validate every component at parse time; mark the listing as
   non-mutating.
4. Add `Engine::set_color`: validate all targets atomically before writing,
   check the active filter for each attribute, write through
   `edit_programmer`, keep the selection, and append wheel choices to the
   message. Add the listing and the `Help` lines.
5. Replace the sheet's `ColorAdd_*`-only swatch with `color::swatch`; add the
   CMY and wheel sheet tests.
6. Document: this design, the roadmap entry, a `PHASE5.md` section, the
   `DESKTOP.md` swatch sentence and an unreleased `CHANGELOG.md` entry.
   Regenerate `COMPLETENESS.md` after adding `color` to the parser keyword list.

## Tests

- **color** — named lookup is case-insensitive and resolves the `Open`/`CTO`
  aliases; hex accepts six digits and rejects short, missing or non-hex forms;
  HSB round-trips the primaries, `360°` and greys; CMY is the inverse of RGB;
  Kelvin gets monotonically warmer as it gets lower, 6500K is near-neutral and
  the range is enforced; additive writes three levels and RGBW extracts white;
  white writes `W 100%` with the primaries at 0; a saturated colour writes
  `W 0`; subtractive writes inverted levels; a wheel prefers an exact name and
  otherwise the nearest set; a tie picks the lowest index; a wheel with no
  named sets fails and lists them; a partial system and a mode with no colour
  fail naming what they have; additive wins over a wheel; `swatch` reads
  additive, RGBW, subtractive and wheel levels; `describe` names the current
  wheel set and prints the systems' components.
- **command** — every form parses to the expected `ColorSpec`; a quoted
  `"#RRGGBB"` is hex, a quoted name is a name; `Color` and `Fixture 1 Color`
  parse as listings; `Color` is non-mutating with no spec and mutating with
  one; rejected inputs name the bad value: short hex, unknown name, `Rgb 300`,
  a missing component, `Hsb 400`, a bare `3200` and `900K`.
- **engine** — `Color Red` on the demo wash writes 100/0/0 and reports
  `#FF0000`; on the CMY demo it writes `C 0 M 100 Y 100`; on the wheel demo it
  picks the nearest set and the message and listing name that set; `Color`
  lists every selected fixture's system and hex; the group form works; a mixed
  selection with one unsupported fixture fails atomically and leaves the
  programmer empty; an intensity filter refuses the colour and names
  `ColorAdd_R`; `Sheet` reports the CMY and wheel swatches; storing a colour in
  a palette keeps the fixture's own system.
- **show** — no format change, so there is no new upgrade test; the existing
  save/load and palette round-trips cover the values the command writes.

## Boundaries

- **No gel library or swatch books.** A gel is a physical filter with a
  spectrum; a three-byte sRGB approximation would claim more than it knows, and
  a vendor library is a data-maintenance project of its own.
- **No colour picker UI.** The command line is the only entry point; item 18's
  GUI gains the corrected swatch, not a picker.
- **No CIE-managed space, chromatic adaptation or spectral mixing.** One sRGB
  space, plainly documented.
- **No automatic CTO/CTB attribute writes.** A Kelvin value is baked into RGB;
  a profile's dedicated `CTO`/`CTB` channel has profile-specific units and
  stays addressable with item 08's set syntax.
- **No attribute-name aliases.** The old sheet swatch also read `Red`,
  `Color_R` and similar as a convenience. The model uses the GDTF standard
  triples only, so `Color` and the swatch can never disagree about what a
  fixture's system is; a hand-built profile should name its channels
  `ColorAdd_*` or `ColorSub_*`.
- **No per-fixture colour calibration.** A fixture's DMX levels are assumed to
  match the desk's sRGB space.
- **No abstract palette colours.** Palettes remain type-keyed physical levels,
  which is what keeps them honest across different colour systems.

## Risks

- **The nearest wheel set can surprise.** Yellow on a red/green/white wheel
  picks whichever is closest, and a tie goes to the lowest index. The command
  message and the listing name the chosen set, and an exact name always wins,
  so the operator can see and correct it.
- **GDTF carries no colour for a set.** Only the set's name is available.
  Unnamed sets are unreachable by colour; the error lists them and item 08
  addresses them by index. The design does not invent a colour from a DMX
  range.
- **RGBW extraction is not power-normalised.** `W = min(r,g,b)` and
  `R -= W` is the classic subtractive extraction; white is one emitter at
  full, not four at full. That matches how desks drive RGBW and is documented,
  but it means a pastel can be brighter than the sum of its parts.
- **sRGB is an approximation for light.** Without colour management a CMY
  inversion and a wheel match are close, not colorimetric. The listing prints
  the hex so the difference is visible rather than hidden.

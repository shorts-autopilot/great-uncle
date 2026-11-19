# 07 · Fixture defaults and `Home`

**Rank** 7 · **Order** 7 · **Size** M · **Show** v8 · **Depends on** 03

## Problem

There is no base look. If no programmer value and no playback value exists for
an attribute, the renderer writes 0 — a colour wheel at DMX 0, a pan at 0, a
gobo at 0. That is not "the fixture's default", it is "whatever the profile
happens to put at byte zero". Every show therefore has to program colour and
position explicitly just to get out of an arbitrary corner, and a cue that
touches only the dimmer drags the colour back to DMX 0.

GDTF already carries the answer: `Channel.Default` (imported as
`Channel.default_raw`, currently unused) and `ChannelFunction` physical ranges.
The engine ignores both at render time.

## Behaviour when this is done

```
Patch Fixture 1 At 1.001 Type "OpenLight Demo Wash" Mode "4ch"
Fixture 1 Default                 home: Dimmer 0% ColorAdd_R 0% … (from the profile)
Fixture 1 Default Dimmer 100
Fixture 1 Default ColorAdd_R 50
Fixture 1 Home                    programmer: Dimmer 100%, ColorAdd_R 50%
```

- A fixture with no programmer and no playback value renders its **home**, not
  zero.
- `Home` recalls the home look into the programmer so it can be stored,
  modified, or used as a starting point.
- `Clear Values` returns the attribute to playback, and playback's absence
  returns it to home.
- Patching seeds home from the GDTF default, so an unedited rig behaves like the
  profile says it should.

## Design

### Show data (v8)

```rust
pub struct Fixture {
    // … existing …
    /// Base look: attribute → normalised level. Empty means "use the profile".
    #[serde(default)]
    pub home: BTreeMap<String, f64>,
}
```

Two levels of default, resolved in one place:

```rust
impl Show {
    /// Stored home, else the channel's GDTF default, else 0.
    pub fn home_level(&self, fixture: &Fixture, attribute: &str) -> f64 {
        if let Some(level) = fixture.home.get_key_value(attribute) { return *level; }
        let Ok((_, mode)) = self.mode_for(fixture) else { return 0.0 };
        let Some(channel) = mode.channel_for(attribute) else { return 0.0 };
        channel.level_from_raw(channel.default_raw)
    }
}
```

`Channel::level_from_raw` is the inverse of `raw_from_level`: it finds the
function whose DMX band contains `default_raw` and maps back to its physical
range. That inverse already exists implicitly in the tests (`dmx_to_defaults…`);
it becomes a real function with its own tests. `default_raw` of 0 in a profile
whose first function starts at 0 maps to 0, so profiles that do not specify a
default keep today's behaviour.

### Rendering

`Stage::level` gains a third fallback, below the programmer and playback:

```
programmer  >  playback  >  home  >  0
```

Effects still modulate the result, masters still scale intensity last, and
blackout (item 06) is still final. This is a one-line change in `Stage` and the
place where the "base look" concept lives.

### Capture and cues

- `store_cue` captures the programmer only, as today. Home is not captured
  unless the operator recalled it, which keeps cues honest: a cue that does not
  mention colour does not freeze the colour.
- `Store Palette` captures the programmer, so a home look must be recalled
  first. This is the documented workflow and needs no special case.

### Commands

| Command | Effect |
|---|---|
| `Fixture 1 Default` | list the effective home (stored and profile) |
| `Fixture 1 Default Dimmer 100` | set one attribute's home |
| `Fixture 1 Default Dimmer Off` | drop the stored override, back to the profile |
| `Group 1 Default Dimmer 100` | set it on every member |
| `Fixture 1 Home` | recall home into the programmer |
| `Group 1 Home` | idem for a group |
| `Home` | recall it for the selection |

`Default` is a noun ("the fixture's default") and `Home` a verb, matching how
the two words are used on desks. `Off` removes the *override*, which is why the
help text says "profile default" for the resulting value.

### Upgrade to v8

Home is empty on upgrade. Because an empty home resolves through
`Channel.default_raw`, the upgrade is behaviour-preserving only for profiles
whose defaults are zero. `load_with_report` therefore lists every fixture whose
computed home has any non-zero attribute:

```
Home seeded from profiles: 3 fixtures have non-zero defaults (fixture 2: ColorAdd_R 100%)
```

That is the honest way to change a base look: say which fixtures move before the
operator sees them move.

## Implementation plan

1. Add `Channel::level_from_raw` with round-trip tests against `raw_from_level`
   for linear and multi-function channels.
2. Add `Fixture.home`, bump to v8, add `Show::home_level`, and the v7→v8
   upgrade with the report line.
3. Use `home_level` as the third fallback in `Stage::level`; extend the render
   tests (home under playback, home under nothing, home with effects).
4. Add `Default`/`Home` parsing, dispatch and listing; `Group` variants reuse
   the group target resolution the intensity commands already have.
5. Add the upgrade note to `docs/PHASE4.md`'s limitations list and a short
   `docs/PHASE5.md` section once implemented.

## Tests

- **fixture_type** — `level_from_raw` inverts `raw_from_level` at 0, 128 and 255
  for a linear channel; for a two-band channel it picks the band containing the
  raw value; an out-of-band raw snaps to the nearest end.
- **show** — a patched fixture with no stored home resolves to the profile
  default; a stored home wins; `default_raw` 0 resolves to 0; v7 upgrade leaves
  home empty and reports non-zero seeds.
- **render** — a fixture with no programmed values renders its home; playback
  overrides home; the programmer overrides both; an effect modulates home;
  masters scale home intensity.
- **engine** — `Fixture 1 Default Dimmer 100; Fixture 1 Home` puts 100% in the
  programmer; `Default … Off` restores the profile value; `Default` lists
  override and effective value; storing the recalled home in a cue keeps it
  after a `Clear Values`; a fixture whose mode lacks the attribute is rejected
  with the attribute named.
- **show round-trip** — home survives save/load.

## Boundaries

- No per-mode home tables (home is per fixture and attribute; a mode change
  keeps the values and the renderer ignores attributes the new mode lacks).
- No "home on power-up" DMX output, no park/default distinction, no per-universe
  default.
- No automatic re-seed when a fixture type is re-imported; that would silently
  change a programmed look.

## Risks

- **Silent look change on upgrade.** Mitigated by the explicit report line and
  by only seeding non-zero defaults; a `--strict-home` escape hatch is
  deliberately not added (another flag to explain, and the report is enough).
- **Home hides an unprogrammed attribute.** A new user may not realise the
  colour comes from the profile. `Sheet` shows the source (`P`, `Pb`, `H`, `-`)
  next to each value, which is also useful for debugging.

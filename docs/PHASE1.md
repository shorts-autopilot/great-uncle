# Phase 1 — engine, show file and DMX output

Phase 1 builds the part everything else stands on: a show that is always valid,
one command path for every input surface, and DMX512 that leaves through a
serial port. It is deliberately headless — no window, no rendering of fixtures,
no playback. The milestone is modest and concrete: **patch two fixtures, type
`At Full`, and see `FF` on the wire**.

## Scope

Done in this phase:

- versioned, self-contained show file with validation and atomic saves;
- fixture types imported from GDTF, including modes and channel functions;
- patch with overlap and universe-bound checks;
- a command language with selection, intensity, patch, import, sheet, status,
  save and load;
- rendering from normalised levels to DMX;
- a 60 Hz engine tick and a 30 Hz frame sender, with blackout on stop;
- four transports: virtual, generic USB-serial, ENTTEC DMX USB Pro, uDMX;
- a console binary with a REPL, a script runner and a frame trace.

Not in this phase (see [Limitations](#limitations)): GUI, playback, effects,
programmer features beyond intensity, 16-bit channels, RDM, show migration.

## Architecture

```
             commands (text)
                    │
        ┌───────────▼────────────┐
        │  Engine::execute       │  single writer, show mutex
        │  lex → parse → dispatch│
        │  mutate → validate     │  rollback on any problem
        └───────────┬────────────┘
                    │ show + revision
        ┌───────────▼────────────┐        ┌────────────────────┐
        │ engine tick (60 Hz)    │───────►│ OutputFrame        │
        │ render only on change  │        │ per-universe slots │
        └────────────────────────┘        └─────────┬──────────┘
                                                    │ copied per frame
                                        ┌───────────▼────────────┐
                                        │ frame sender (30 Hz)   │
                                        │ one transport/universe │
                                        └────────────────────────┘
```

Three rules hold the design together:

1. **One writer.** The GUI, the CLI, scripts and (later) OSC/Lua all produce the
   same command text and hand it to `Engine::execute`. Behaviour cannot drift
   between surfaces, and there is never a second mutator of show state.
2. **The show is always valid.** Every mutation goes through `Engine::mutate`,
   which clones the show, applies the change, validates the result and restores
   the clone if anything is wrong. Invalid state can never reach the renderer,
   the saver or a later `Load`.
3. **Output only reads.** The two output threads borrow the show under a mutex,
   render into an owned frame, and never mutate. A transport failure is counted
   and reported; it never takes the console down.

### Crates

| Crate | Library | Responsibility |
|---|---|---|
| `crates/dmx` | `dmx` | wire formats, serial port, USB, virtual loopback |
| `crates/engine` | `engine` | show, fixture types, GDTF, commands, render, runtime |
| `crates/console` | binary `openlight` | argument parsing, REPL, scripts, trace |

## Show file

`openlight-show`, version 1, JSON. A show embeds its fixture types, so opening
it on another machine never depends on a local fixture library.

```json
{
  "format": "openlight-show",
  "version": 1,
  "name": "Untitled Show",
  "fixture_types": [ … ],
  "universes": [{ "number": 1, "label": "Universe 1", "size": 512 }],
  "fixtures": [{
    "number": 1, "name": "Demo Wash", "type_key": "OpenLight Demo Wash",
    "mode": "4ch", "universe": 1, "address": 1,
    "levels": { "Dimmer": 1.0, "ColorAdd_R": 0.0 }
  }]
}
```

- Unknown fields are rejected (`deny_unknown_fields`), so a typo is an error
  rather than silently ignored data.
- The version is checked strictly. There is no migration machinery yet, and a
  future version is refused with a message naming the supported version.
- Loading validates before returning: universe numbers and sizes, duplicate
  fixture numbers and type keys, mode footprints, channel offsets, patch
  overruns, overlapping patches, and levels outside `0..=1`.
- Saving validates again, writes `<name>.tmp`, `fsync`s it, then renames it over
  the target. A crash mid-save leaves the previous show intact.

Programming is stored as a **normalised physical level per attribute**
(`0.0..=1.0`), not as a DMX byte. Rendering maps it through the channel
functions of the fixture mode, so a more accurate curve in a profile
immediately applies to existing programming.

## Fixture types and GDTF

`Import Gdtf "file.gdtf"` reads the `description.xml` inside the archive:

| GDTF | Used for |
|---|---|
| `FixtureType@Manufacturer`, `@Name` | library key (`OpenLight Demo Wash`) |
| `Attributes/Attribute` | pretty names, feature, unit |
| `DMXModes/DMXMode` | modes and their footprints |
| `DMXChannel@Offset`, `@DMXBreak`, `@Default` | channel position and power-on value |
| `LogicalChannel@Attribute` | the attribute a channel drives |
| `ChannelFunction` + `DMXFrom`/`DMXTo`/`PhysicalFrom`/`PhysicalTo` | physical ↔ DMX mapping |

`DMXValue` is read in its `byte/value` form; a bare number is a byte-0 value.
Channels on a DMX break other than 1 are skipped with a warning, as are 16-bit
functions (their low byte is used). Mutually exclusive logical channels on one
offset keep the first and warn about the rest. Nothing about a profile is
invented: what the importer cannot represent is reported, not guessed.

## Command language

Every line is one or more `;`-separated statements. Keywords are
case-insensitive; names and paths are quoted when they contain spaces.

| Command | Meaning |
|---|---|
| `Fixture 1 Thru 5` | select fixtures (also `1 Thru 5`, `1 + 3`) |
| `Fixture 1 Thru 5 At 50` | set Dimmer to 50% |
| `At 50` / `Full` / `Off` | apply to the current selection |
| `Clear` | clear the selection |
| `Patch Fixture 1 At 1.001 Type "X" [Mode "4ch"]` | patch or repatch |
| `Unpatch Fixture 1 Thru 3` | remove fixtures |
| `Import Gdtf "file.gdtf"` | import fixture types |
| `Sheet` | fixture sheet: patch and levels |
| `Status` | output state, counters, show summary |
| `Save "show.json"` / `Load "show.json"` | write or read a show |
| `Help` | command list |

Addresses are `universe.address`, e.g. `1.001` is universe 1, address 1. Patching
checks that the footprint fits the universe and that no other fixture overlaps
those slots; both are also enforced on load.

Errors carry a hint when there is an obvious next step, for example:

```
openlight> At 50
error: nothing selected (select fixtures first, e.g. `Fixture 1 Thru 4`, or name them: `Fixture 3 At 50`)
```

Addressing fixtures (`Fixture 1 At Full`) leaves them selected, so a following
`Full` or `Off` applies to the same set.

## Console

```
openlight [options]

  --show <file>      load a show at start
  --gdtf <file>      import a GDTF profile (repeatable)
  --output <spec>    bind a universe, e.g. 1=virtual, 1=generic:/dev/ttyUSB0
  --script <file>    run commands from a file, one per line
  --frames <n>       run for n output frames, then exit
  --fps <n>          frame rate (default 30)
  --engine-hz <n>    engine tick rate (default 60)
  --trace            print the last virtual frame
```

Without `--script` and `--frames` it opens a REPL. `Status` is printed before
shutdown, so the run ends with the frame counters that were actually achieved.

## Output transports

| Spec | Wire | Notes |
|---|---|---|
| `virtual` | in-memory | records frames; used by tests, demos and `--trace` |
| `generic:/dev/ttyUSB0` | DMX512 direct | 250000 baud, 8N2, break 176 µs, MAB 12 µs |
| `enttec:/dev/ttyUSB0` | Widget API 1.44 | 57600 8N1, label 6, `0x7E … 0xE7` |
| `udmx` | USB control | `16c0:05dc`, vendor requests 1 and 2 |

**Generic serial.** The host generates the whole frame: `TIOCSBRK`, hold for
176 µs, `TIOCCBRK`, mark-after-break pause of 12 µs, then one `write` of the
start code and 512 slots, then `tcdrain`. 250000 baud is not a standard termios
speed, so the port is configured with `termios2` and `BOTHER` rather than a
serial crate. If the link cannot drive break (a pty, some adapters), the
transport records one warning and keeps sending instead of failing.

**ENTTEC.** `7E | label | len LSB | len MSB | 00 + 512 slots | E7`, length 513,
one write per frame, `drain` afterwards. Label 100 addresses universe 1 on DMX
King units in enhanced mode.

**uDMX.** Request 2 sends a channel range: `wValue` = channel count, `wIndex` =
zero-based first channel, payload = values, chunked to 64 bytes because a
full-speed control transfer cannot carry more. Request 1 writes a single
channel (`wValue` = value, `wIndex` = channel).

Unbound universes are still rendered but not transmitted; a universe bound to a
transport that is missing gets an error counter, not silence about it.

## Evidence

`cargo test --workspace` runs **83 tests**: 11 in `dmx`, 72 in `engine`. No
hardware is required.

- **Wire formats.** The ENTTEC frame is checked byte for byte (`7E 06 01 02 00
  … E7`), the uDMX range parameters and chunk sizes are checked against a mock
  USB device, and the generic transport is checked for one configuration, one
  break and one write per frame.
- **Real character device.** `serial::pty_tests` opens a pseudo terminal, drives
  the same code path as a USB adapter (open, `termios2`/`BOTHER` at 250000 8N2,
  break attempt, one write, `tcdrain`) and reads the bytes back from the master
  side: start code first, then the slot values at the expected offsets. It also
  proves the break-tolerant path: a pty rejects `TIOCSBRK`, the transport warns
  once and still delivers the frame.
- **Engine.** 72 tests cover lexing and parsing (including error hints), patch
  overlap and overrun rejection, rollback on rejected commands, level mapping
  through channel functions, sheet and status output, save/load round-trips,
  strict version and unknown-field rejection, GDTF parsing (multi-function
  channels, break skipping, 16-bit warnings, real zip archives), rendering, and
  the output runtime (frames reaching the transport, live changes being picked
  up, blackout on stop, status counters, unbound universes).
- **Milestone.** `examples/phase1-demo.txt` imports the demo profile, patches
  two washes at `1.001` and `1.005`, takes them to `Full`, prints the sheet and
  status, saves the show, and the trace shows the transmitted frame:

  ```
  Universe 1: 20 frames, last frame 513 bytes, start code 0x00, 2 non-zero slots
    00 FF 00 00 00 FF 00 00 …
  ```

## Limitations

Deliberate for this phase, and worth knowing before the next one:

- **No GUI.** The Tauri shell is Phase 4; everything here is the engine and a
  terminal surface.
- **8-bit output only.** 16-bit channel pairs, fine channels and `DMXValue`
  byte 1 are imported as warnings and treated as their low byte.
- **No channel sets.** Shutters, colour wheels, gobo indexing and other
  multi-range attributes have their channel functions imported, but the
  programmer only writes a continuous level; there is no named-set syntax yet.
- **Multiple DMX breaks are skipped**, not merged, and RDM is absent.
- **Linux only for real serial ports.** `termios2`, `BOTHER` and
  `TIOCSBRK`/`TIOCCBRK` are Linux interfaces; other platforms build and run the
  virtual transport but refuse a serial port with a clear message.
- **No hardware in the loop.** The frame timing is exercised through a pseudo
  terminal and a recording link, which proves ordering and framing but cannot
  prove electrical DMX timing (break width, MAB, refresh stability on a real
  adapter). That needs a scope or a DMX analyser.
- **uDMX is untested on hardware.** Its protocol is covered by unit tests
  against a mock device only.
- **Channel defaults are stored, not applied.** `DMXChannel@Default` is imported
  into the fixture type, but patching starts every attribute at 0 (dark) so a
  freshly patched rig never surprises anyone.
- **No show migration.** Version 1 is the only readable format; a future
  version is refused rather than upgraded.
- **Per-universe rate is shared.** All transports are sent from one 30 Hz loop,
  so a slow transport delays the others on that tick.

## What Phase 2 builds on

- `Engine::mutate` and the command text are the seam for the GUI: it will send
  the same strings, and get the same validation and rollback.
- `Registry`/`NameIndex` with generation-checked handles already exist for
  objects that are not user-numbered (fixture types now; pools, cue lists and
  executors next).
- The engine tick is the place for fades and effects: it re-renders when the
  revision changes, so a time-driven revision bump is all a fade needs.
- `OutputFrame` is per universe, so adding universes or a second transport is a
  binding change, not a renderer change.

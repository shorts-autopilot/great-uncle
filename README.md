# OpenLight

A lighting console for DMX512 rigs, built as a native desktop application. The
engine, the show file and the command language are written from scratch in
Rust; output goes to a **serial port** rather than over the network, so a plain
USB-serial adapter is enough.

OpenLight is an independent project. It is not affiliated with, endorsed by, or
derived from any console manufacturer, and it ships no third-party fixture
profiles, icons, manual text or artwork. Fixtures are read from open GDTF
profiles that you supply.

## Status

| Phase | Scope | State |
|---|---|---|
| 0 | Manual crawl → feature specification | done — see [`spec/REPORT.md`](spec/REPORT.md) |
| 1 | Engine, show file, command line, DMX output | done — see [`docs/PHASE1.md`](docs/PHASE1.md) |
| 2 | Palettes, cues, sequences, executor faders | done — see [`docs/PHASE2.md`](docs/PHASE2.md) |
| 3 | Groups, worlds, filters, masters, effects | done — see [`docs/PHASE3.md`](docs/PHASE3.md) |
| 4 | Timecode, move-in-black, undo, first UI shell | done — see [`docs/PHASE4.md`](docs/PHASE4.md) |
| 5+ | desk state, output, journal, blackout/lock | in progress — see [`docs/ROADMAP.md`](docs/ROADMAP.md) |
| 5+ | triggers, timing, GUI, … | designed — see [`docs/ROADMAP.md`](docs/ROADMAP.md) |

It is a console with a command line, a full-screen terminal shell, a script
runner and four output transports. You can program, store palettes and cues, run
a cue list from an executor fader, address the rig through groups, fence it off
with worlds and filters, scale it with grand, rate and group masters, animate it
with effects, run cues from a timecode show, move fixtures in black and undo any
show edit. The programmer, the faders and the playback positions live in a desk
file written beside the show (`look.olshow` + `look.oldesk`), so a document
never carries someone else's half-finished look. `Blackout` darkens the rig
without touching the programming, `Lock` refuses changes until it is released,
and every universe reports the frame rate and send time it actually achieved.
Every edit is journaled beside the show as it happens and a snapshot is written
on a timer, so a power cut costs the last few seconds rather than the session:
the next start replays the journal and offers to save. There is no desktop GUI
yet.

## Quick start

```bash
cargo test --workspace            # 219 tests, no hardware needed
cargo run -p openlight-console    # interactive console
cargo run -p openlight-console -- --ui   # full-screen terminal shell
```

Light two fixtures on a virtual universe:

```bash
python3 tools/make_demo_gdtf.py   # writes examples/gdtf/openlight-demo-wash.gdtf
cargo run -p openlight-console -- \
  --output 1=virtual \
  --script examples/phase1-demo.txt \
  --frames 20 --trace
```

Store a palette and two cues, then run them from a fader at 75%:

```bash
cargo run -p openlight-console -- \
  --output 1=virtual \
  --script examples/phase2-demo.txt \
  --frames 120 --trace
```

```
Executor 1 → cue 2 "High" (fade 1s, delay 0s)
Universe 1: 120 frames, last frame 513 bytes, start code 0x00, 6 non-zero slots
  00 BF FF 00 4D BF FF 00 4D …
```

`BF` is a 100% dimmer through the 75% master fader, `FF` is red and `4D` is the
30% blue stored in the palette.

Address four fixtures through a group, fence two off with a world, program
through an intensity filter, scale the rig with masters and pulse it with an
effect:

```bash
cargo run -p openlight-console -- \
  --output 1=virtual \
  --script examples/phase3-demo.txt \
  --frames 120 --trace
```

```
Universe 1: 120 frames, last frame 513 bytes, start code 0x00, 4 non-zero slots
  00 BF 00 00 00 BF 00 00 00 BF 00 00 00 BF 00 …
```

`BF` is 75% through the grand master on each of the four dimmers.

Run a cue list from a timecode show, with move-in-black and undo:

```bash
cargo run -p openlight-console -- \
  --output 1=virtual \
  --script examples/phase4-demo.txt \
  --frames 120 --trace
```

```
Universe 1: 120 frames, last frame 513 bytes, start code 0x00, 4 non-zero slots
  00 FF FF 00 00 FF FF 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 …
```

Both fixtures are at full with red at full: cue 2 of the timecode show has run.

The trace line ends with the first slots of the frame that went out:

```
Universe 1: 20 frames, last frame 513 bytes, start code 0x00, 2 non-zero slots
  00 FF 00 00 00 FF 00 00 …
```

`00` is the DMX start code, `FF` at address 1 and 5 is the dimmer of each wash.

Send the same look to real hardware:

```bash
# bare USB-serial adapter (FTDI/CH340), 250 kbaud 8N2 with break and MAB
cargo run -p openlight-console -- --output 1=generic:/dev/ttyUSB0 \
  --script examples/phase1-demo.txt

# ENTTEC DMX USB Pro (or DMX King in Pro-compatible mode)
cargo run -p openlight-console -- --output 1=enttec:/dev/ttyUSB0
```

## Layout

```
crates/dmx/       DMX512 transports: virtual, generic serial, ENTTEC, uDMX
crates/engine/    show file, fixture types, GDTF import, commands, levels, palettes,
                  playback, pools (groups/worlds/filters), effects, render,
                  output runtime
crates/console/   the `openlight` binary: CLI, REPL, terminal shell, script runner
examples/         demo show scripts and their synthetic GDTF profile
tools/            manual crawler (Phase 0), status stamping, demo profile generator
spec/             the Phase 0 feature specification, statuses and evidence
docs/             per-phase design notes
```

## Documentation

- [`docs/PHASE1.md`](docs/PHASE1.md) — engine, show file, command line, output
  transports, test evidence, known limitations.
- [`docs/PHASE2.md`](docs/PHASE2.md) — programmer vs playback, tracking, palettes,
  cues, sequences, executor faders, show format v2, limitations.
- [`docs/PHASE3.md`](docs/PHASE3.md) — groups, worlds and filters, grand/rate/group
  masters, effects, show format v3, limitations.
- [`docs/PHASE4.md`](docs/PHASE4.md) — undo/redo, move-in-black, timecode, the
  terminal UI shell, show format v4, limitations.
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — the ranked work after Phase 4, with a
  design in [`docs/design/`](docs/design/) for every item.
- [`spec/matrix.md`](spec/matrix.md) — the Phase 0 feature matrix this work is
  measured against, with per-row status.
- [`spec/SCHEMA.md`](spec/SCHEMA.md) — how the crawled specification is stored.

Row status is maintained by `python3 tools/spec_status.py`; `--check` fails when
`matrix.json`/`matrix.md` no longer match the recorded statuses.

## Licence

MIT.

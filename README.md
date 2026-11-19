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
| 2+ | Programming, playback, effects, UI | not started |

Phase 1 is a headless console: a CLI with a REPL, a script runner and four
output transports. There is no GUI yet.

## Quick start

```bash
cargo test --workspace            # 83 tests, no hardware needed
cargo run -p openlight-console    # interactive console
```

Light two fixtures on a virtual universe:

```bash
python3 tools/make_demo_gdtf.py   # writes examples/gdtf/openlight-demo-wash.gdtf
cargo run -p openlight-console -- \
  --output 1=virtual \
  --script examples/phase1-demo.txt \
  --frames 20 --trace
```

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
crates/engine/    show file, fixture types, GDTF import, commands, render, output runtime
crates/console/   the `openlight` binary: CLI, REPL, script runner
examples/         a demo show script and its synthetic GDTF profile
tools/            manual crawler (Phase 0) and the demo profile generator
spec/             the Phase 0 feature specification and evidence
docs/             per-phase design notes
```

## Documentation

- [`docs/PHASE1.md`](docs/PHASE1.md) — architecture, command reference, output
  transports, test evidence, known limitations.
- [`spec/matrix.md`](spec/matrix.md) — the Phase 0 feature matrix this work is
  measured against.
- [`spec/SCHEMA.md`](spec/SCHEMA.md) — how the crawled specification is stored.

## Licence

MIT.

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
| 5+ | desk state, output, journal, blackout/lock, cues, timing, home, cue editing, viewing, 16-bit, patch tools, effects, external timecode, remote, MVR, fixture builder, macros/CLI, release readiness, desktop GUI | done — all eighteen roadmap designs built; see [`docs/PHASE5.md`](docs/PHASE5.md) |

It is a console with a command line, a full-screen terminal shell, a script
runner and four output transports. You can program, store palettes and cues, run
a cue list from an executor fader, address the rig through groups, fence it off
with worlds and filters, scale it with grand, rate and group masters, animate it
with effects — several attributes per effect, each with its own size and phase,
and musical rates that follow `Tempo` — run cues from a timecode show with
several tracks and markers, follow MIDI Time Code from a serial port or positions
from OSC, press Go from an OSC controller or the browser remote, import an MVR
rig and fix or create its fixture profiles, move
fixtures in black and undo any show edit. `Tempo 128` or a few `Tap`s set the
desk tempo; `Rate 1/4` retimes a running effect in beats, while `Rate 2hz` stays
a free rate. The programmer, the faders and the playback positions live in a
desk file written beside the show (`look.olshow` + `look.oldesk`), so a document
never carries someone else's half-finished look. `Blackout` darkens the rig
without touching the programming, `Lock` refuses changes until it is released,
and every universe reports the frame rate and send time it actually achieved.
Every edit is journaled beside the show as it happens and a snapshot is written
on a timer, so a power cut costs the last few seconds rather than the session:
the next start replays the journal — starting after the last successful save,
and never on top of a snapshot taken while the journal was off — and offers to
save. A cue list can also run itself: a cue stores a `Go`, `Follow` or `Time`
trigger, and an executor's button and fader are `Go`, `Fader`, `Flash` or
`Solo`, with their positions kept in the desk file. A cue's fade can be split
per attribute — colour in half a second, position in four, a snap on the
dimmer — with a curve on each lane and a stagger between consecutive fixtures.
A fixture also has a home look: with nothing programmed it renders the profile's
`Default` rather than DMX zero, `Default` edits and lists it, and `Home` recalls
it into the programmer; `Sheet` says whether each value comes from `P`, `Pb`,
`H` or nowhere. A cue can stop tracking too: `Cue 2 Block` holds the whole
state so later edits upstream cannot move it, `Cue 2 Shield Pan` asserts one
attribute at a fixed level, and `Cue 2 Show` prints the absolute look. `Update
Cue 2` merges the programmer into a stored cue, `Store Cue 2 Replace` keeps only
what the programmer has, and `Cue 2 Move` / `Copy` renumber or copy a cue
without changing any look. A blind desk edits the programmer without sending it:
`Blind` keeps the programmer off the stage while playback keeps running, and
`Blind Off` shows the edit in one frame. `Preview Cue 14` renders a stored cue
without sending it and prints its first non-zero channels in `Sheet`, while
`Highlight` holds the selected fixtures at full intensity as an output aid that
can never be stored. A profile's fine DMX slot is real precision now: a 16-bit
attribute renders two bytes, and a channel's named sets are addressable by
number or exact name (`Fixture 1 Gobo 2`, `Fixture 1 Gobo "Rotate CW"`), with a
listing that shows every band and where the wheel currently lands. Patching a
whole rig no longer means typing every address: `Clone Fixture 1 At 1.101 Count
8` copies a fixture across a universe atomically, `Patch Check` reports overlaps,
gaps and footprint overflow, and `Patch Import` / `Report Patch` round-trip the
patch through a spreadsheet. `Report Cues`, `Report Palettes`, `Report Status`
and `Report Show` write the listings as files, and `--check` runs the patch
check for CI. A native window (`cargo run -p openlight-desktop --features gui`)
draws the same five panes and command line, with draggable executor faders and
a blackout button; the terminal shell stays the default for headless boxes, and
the window build is feature-gated so tests and the MSRV job need no display.

## Quick start

```bash
cargo test --workspace            # 495 tests, no hardware needed
cargo run -p openlight-console    # interactive console
cargo run -p openlight-console -- --ui   # full-screen terminal shell
cargo run -p openlight-desktop --features gui   # native window
cargo run -p openlight-desktop -- --headless-smoke  # load, snapshot, exit (no display)
```

## Installing a release

`packaging/make-tarball.sh` writes `dist/openlight-<version>-<target>.tar.gz`
with both binaries (`openlight` and `openlight-desktop`), this README, the docs,
the examples and the udev rules. `GUI=0` builds the console only. From an
extracted tarball:

```bash
sudo install -m 0755 openlight /usr/local/bin/openlight
[ -f openlight-desktop ] && sudo install -m 0755 openlight-desktop /usr/local/bin/openlight-desktop
sudo install -m 0644 packaging/udev/60-openlight.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo usermod -aG dialout "$USER"   # log out and back in for the group change
openlight --probe                   # what each attached port actually does
```

The udev file covers FTDI, CH340, Prolific, Arduino ACM and Anyma uDMX devices.
Adjust the group if your distribution does not use `dialout`.

## Platforms and output

| Platform | Output | Status |
|---|---|---|
| Linux | `generic:` serial, `enttec:`, `udmx`, `virtual` | supported, primary |
| macOS | `generic:` serial, `enttec:`, `virtual` | supported; the serial backend is compile-checked, not yet run on hardware |
| Windows | `virtual` | experimental; there is no serial backend yet, so `generic:` and `enttec:` both report `unsupported` |
| any | `virtual` | supported everywhere and used by every test |

The serial layer is `termios2` on Linux and `termios` + `IOSSIOSPEED` on macOS.
On a platform without a serial backend the workspace still builds and every
serial transport reports `unsupported`; only *device* support varies. ENTTEC is
the path to add first on a new platform because the widget owns the frame
timing, but today it uses the same serial layer as `generic:`.

`openlight --probe` sends three frames to every detected serial port (and every
`--output`) and reports the achieved rate, break/MAB and jitter without starting
the output threads. `Probe` inside the shell does the same and skips ports the
runtime already owns. The command form sends synchronously and holds the engine
while it runs, so it is an engineering action rather than something to type
during a show. It is the measurement a bug report should carry; treat the
support matrix above as exactly what has been measured, not more.

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
Universe 1: last frame 513 bytes, start code 0x00, 2 non-zero slots
  00 00 FF 00 00 00 FF 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 …
```

Both fixtures have red at full while the dimmer is still dark: the
per-attribute cue has done its colour but its delayed dimmer has not started.

## OSC and browser remote

The console listens for OSC on UDP 9000 and serves the browser remote on TCP
8080, both on loopback by default. The first start creates
`~/.openlight/remote.token`; open `http://127.0.0.1:8080`, paste that token once,
and the page shows executors, masters, blackout and a command box. OSC uses the
same command line:

```
/openlight/executor/1/go
/openlight/executor/1/fader 0.75
/openlight/blackout 1
/openlight/timecode 12.5
```

Use `--osc-port`, `--remote-port`, `--remote-bind` and `--remote-token-file` to
move them, or `--no-remote` for a scripted or headless run. Plain HTTP is for a
trusted show network; the default bind is loopback and the token is never
printed in logs.

## Command line as a product

The console now has the flags a script or a CI job needs: repeatable `--command`,
`--script -` for stdin, `--check [file]` for a stable `path: problem` result,
`--status --json` for machine-readable state, `--version`, `--quiet`, `--log`
and shell completions. Commands run before the script, in the order given.

```bash
openlight --version                 # package, git hash and show format
openlight --probe
openlight --check look.olshow
openlight --command "Fixture 1 At 50" --command "Store Cue 1" --frames 10
openlight --script - < examples/phase1-demo.txt
openlight --status --json
openlight --completions bash
```

`--probe` (and `Probe`) reports the DMX framing each port actually achieves, so
a hardware problem can be described with numbers. `--version` includes the
short git hash when the binary was built in a checkout. Unsafe archive paths,
oversized archives or documents and too many channels are refused before
parsing, naming the limit that was exceeded.

`Begin [label]` and `End` group edits into one undo step. `Macro 1 "cue" Blackout; Wait 2; Blackout Off` stores statement text; `Macro 1 Run` expands it through the same command path, with cycle and depth guards. `Wait` is accepted while editing a macro but refused when it runs, because the engine must not block its output tick. Bulk imports open a group internally.

## MVR and the fixture builder

Import a rig before the first script line with `--mvr`, or from the command
line; profiles travel inside the MVR archive and each fixture keeps its MVR
address and name:

```bash
cargo run -p openlight-console -- --mvr "rig.mvr" \
  --output 1=virtual --script examples/phase2-demo.txt --frames 120 --trace
```

```
Imported 24 fixtures, 3 types, 24 GDTF archives from rig.mvr
```

Profiles are editable data. `Type New` starts a small profile, `Mode` and
`Channel` build it, and `Import Type`, `Export Type` and `Copy Type` move it
through the portable `.olft` JSON format:

```
Type New "Generic 4ch"
Type "Generic 4ch" Mode "4ch"
Type "Generic 4ch" Channel 1 Dimmer "Dimmer"
Type "Generic 4ch" Channel 2 ColorAdd_R "Red"
Export Type "Generic 4ch" "generic-4ch.olft"
Copy Type "Generic 4ch" As "Generic Wide"
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
crates/engine/    show file, fixture types, GDTF import, commands, levels, palettes,
                  playback, pools (groups/worlds/filters), effects, render,
                  output runtime, structured listings
crates/app/       wiring shared by every front end: load, recover, runtime, snapshots
crates/console/   the `openlight` binary: CLI, REPL, terminal shell, script runner
crates/desktop/   the `openlight-desktop` window (feature `gui`) and its headless model
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
- [`docs/DESKTOP.md`](docs/DESKTOP.md) — the window's crate split, the toolkit
  spike and its measurements, and what the GUI deliberately does not do.
- [`CHANGELOG.md`](CHANGELOG.md) — released and unreleased changes.
- [`SECURITY.md`](SECURITY.md) — how to report a problem and what the desk lock
  does not protect.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — the one-writer rule and the test
  standard.
- [`docs/COMPLETENESS.md`](docs/COMPLETENESS.md) — every Phase 0 manual row
  classified, every out-of-scope row with a reason, and the chapters that still
  need designs.
- [`spec/matrix.md`](spec/matrix.md) — the Phase 0 feature matrix this work is
  measured against, with per-row status.
- [`spec/SCHEMA.md`](spec/SCHEMA.md) — how the crawled specification is stored.

Row status is maintained by `python3 tools/spec_status.py`; `--check` fails when
`matrix.json`/`matrix.md` no longer match the recorded statuses.

## Licence

MIT.

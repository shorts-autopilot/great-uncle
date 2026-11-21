# Contributing

## One writer, one grammar

Every input surface — the terminal shell, a macro, OSC, the web remote, a
future GUI — turns input into command text and calls `Engine::execute`. Do not
add a second path into the show and do not add a second renderer: listings and
reports come from the same functions the command line prints. A feature that
only works in one surface is not finished.

The show file is what an operator programmed; the desk file is where the
handles are; session state is never saved. Before adding a field, decide which
of those three owns it. The rule: if losing it would only mean moving a fader
back, it is desk state; if losing it would mean reprogramming, it is show
state.

## Before you send a change

The same commands CI runs:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
python3 tools/spec_status.py --check
tools/demo_trace.sh check
```

The test standard is the one in [`docs/ROADMAP.md`](docs/ROADMAP.md):

- unit tests in the module that owns the behaviour, driven by explicit
  instants, never `sleep`;
- one engine-level test through `Engine::execute` for a new command;
- a rejected-input test that names the value that was wrong;
- a save/load round-trip whenever the format changes, plus an upgrade test from
  the previous version;
- no new `unsafe` outside `crates/dmx`.

No test may need DMX hardware. The virtual transport is the default for a
reason; a pseudo terminal is as close as CI gets to a serial port.

## Demos and documentation

`examples/*.txt` are runnable demos. If a change alters what one prints, record
the new trace with `tools/demo_trace.sh record` and include the changed
`examples/expected/*.trace` in the same commit. A rendering regression should
fail a build rather than a rehearsal.

Update `README.md`, `docs/PHASE5.md` and `docs/ROADMAP.md` when a design lands,
and regenerate the specification matrix if a row status changed.

## Commits

Keep a commit to one design. Explain why the change exists, not just what
changed. The workspace MSRV is tested in CI; do not raise it casually, and do
not add a dependency without checking its licence (`cargo deny check`) and its
rust-version.

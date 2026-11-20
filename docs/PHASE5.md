# Phase 5 — a desk that can be left running

Phase 4 closed the first loop: cues, timecode, move-in-black and undo. Phase 5
is the work that makes the desk safe and expressive in a real room: the show and
the console get separate owners, output has its own clock and telemetry, every
edit survives a crash, the rig can be blacked out and locked, cue lists run
themselves, fades can be split per attribute, and a fixture has a base look
instead of DMX zero.

Each item landed with its own tests, in the order the designs ask for.

## Show data and desk state (03)

`look.olshow` stores programming; `look.oldesk` stores the handles: selection,
programmer, masters, executor fader positions and playback positions. `Save`,
`Save Show`, `Save Desk` and `Load Show` address them independently. A v4 file's
programmer and master positions are handed to the desk on load. Show v5.

## Output scheduling, telemetry and the watchdog (05)

Each universe has a self-clocked worker, so a slow universe cannot stall
another. `Status` reports measured `achieved_hz`, frames, errors, reconnects,
send time and staleness. A broken transport reconnects with backoff; a stalled
engine fades that universe to black rather than freezing the last look.

## Journal, autosave and replay (04)

Every edit is appended to `<show>.journal` as a statement and replayed through
the ordinary command path. `<show>.autosave` is a full snapshot written on a
timer and after every save. `--autosave`, `--journal` and `--no-recover` control
them.

Replay starts after the newest thing the document already contains:

- a successful `Save` writes a checkpoint naming the canonical show path;
- a snapshot records the journal revision it was taken at, including when the
  journal was switched off (the console reads the file's head at startup);
- a snapshot written by an older build without a revision is treated as the
  journal's new head, so its stale tail is never replayed onto it.

A journal that lost its head to rotation or damage starts where the file starts
and says so; a break in the revision chain stops replay.

## Blackout and lock (06)

`Blackout [Fade s]` and `Blackout Off` scale intensity without touching colour,
position or the programming; lifting the blackout is always allowed. `Lock
[code]` / `Unlock [code]` refuse mutating commands through the `mutates()`
allow-list.

## Cue triggers and executor state (01)

A cue stores `Go`, `Follow t` or `Time t`; `Cue n Trigger …` retimes it without
restoring values. `Follow` counts from the settled fade and `Time` from the cue
start, with no wrap past the last cue. Executors gain `Mode Go | Fader | Flash |
Solo`, and their fader, flash and solo handles live in the desk file. Show v6.

## Per-attribute timing, curves and stagger (02)

A cue can override its fade and delay per attribute, give each lane a `Linear`,
`EaseIn`, `EaseOut`, `EaseInOut` or `Snap` curve, and stagger consecutive
fixtures.

```
Store Cue 2 Fade 3
Cue 2 Timing ColorAdd_R Fade 0.5
Cue 2 Timing Dimmer Delay 2.5 Curve Snap
Cue 2 Stagger 0.2
Sequence 1
```

```
Cue 2  Fade 3.0s
  Timing  ColorAdd_R       fade 0.5s  delay 0.0s  Linear
  Timing  Dimmer           fade 3.0s  delay 2.5s  Snap
  Stagger 0.20s between fixtures
```

Internally a run carries one lane per changing attribute; a cue whose lanes all
match its own fade and delay at `Linear` keeps the older single-blend
arithmetic. Pause, resume and a hand on an executor fader rebase the lanes so a
held fade continues where it stopped. A cue settles when its slowest lane does,
so a quick colour fade cannot fire a `Follow` while a long move is still
running. Show v7.

## Fixture defaults and `Home` (07)

The renderer's base look is now `programmer > playback > home > 0`. Home is a
per-fixture map with the profile's `Channel.Default` as its fallback, so an
unedited rig comes up the way the GDTF profile describes instead of at DMX zero.

```
Fixture 1 Default                     list the effective home
Fixture 1 Default Dimmer 100          store an override
Fixture 1 Default Dimmer Off          drop it, back to the profile
Group 1 Default ColorAdd_R 50         the same for every member
Fixture 1 Home / Home / Group 1 Home  recall home into the programmer
```

`Sheet` prints the source next to each value: `P` programmer, `Pb` playback, `H`
home, `-` nothing. Cues still capture the programmer only, so a cue that does
not mention colour does not freeze it. Loading a pre-v8 show reports every
fixture whose profile default is not zero before the operator sees it move.
Show v8.

## Known limitations

- **No DMX hardware in the test environment.** Timing is exercised against a
  pty and a virtual transport; electrical timing and real serial adapters remain
  unproven.
- **Recovery is path-based.** A snapshot's revision belongs to the journal it
  was taken with; moving a show and its journal to another directory is not a
  supported recovery workflow.
- **Home is per fixture and attribute.** There is no per-mode home table, no
  park/default distinction and no automatic re-seed when a profile is
  re-imported.
- **Only seven of the eighteen roadmap designs are built.** The remaining
  eleven are listed in [`ROADMAP.md`](ROADMAP.md).

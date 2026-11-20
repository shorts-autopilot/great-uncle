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

## Cue-editing safety (10)

A cue can stop tracking: `Cue n Block` materialises the state at that cue and
stores it whole, so later edits to earlier cues no longer move it; `Unblock`
turns it back into a delta. `Cue n Shield <attribute>` asserts one attribute at
the level it has at that cue and stores that level, so the shield survives the
very upstream edit it exists to block; `Cue n Shield` lists what is shielded and
`Cue n Shield <attribute> Off` removes the assertion. Both are visible in the
`Sequence` listing and in `Cue n Show`, which prints the cue's absolute state —
the look it actually produces, not the stored delta.

`Update [Cue n] [Sequence n]` merges the programmer's touched attributes into a
cue, defaulting to the cue that sequence is running, without retyping the list.
`Store Cue n` now merges by default, keeping values the cue already held;
`Store Cue n Replace` keeps exactly what the programmer has. `Cue n Move m` and
`Cue n Copy m [Sequence s]` renumber or copy a cue, timing and shields included,
and `Sequence::rebase` rewrites every delta so no cue's look moves. `Delete Cue`
refuses when a timecode event starts the cue and names the event and position
instead of failing with a generic validation error. Show v9.

## Blind, preview and highlight (09)

`Blind` keeps the programmer off the stage without a second buffer: `At`,
`Store` and `Update` still work and `Sheet` still lists the values, but the
renderer substitutes an empty programmer for as long as blind is on. Playback
keeps running, so editing during a show does not freeze it, and `Blind Off`
makes the edit appear in one frame. Blind is saved with the desk, and `Status`
and the shell header both carry a marker.

`Preview Cue 14 [Sequence 2]` renders that cue's tracked state with no
programmer, effects or masters and prints its first non-zero channels in
`Sheet`; a bare `Preview` inspects the next cue of the running sequence, and a
single-sequence show needs no `Sequence` clause. The frame is only a listing
value, never sent. `Preview Off` clears it, deleting the cue clears it with a
message, and moving the cue carries it to the new number.

`Highlight` holds every selected fixture at full intensity in the renderer,
after the programmer/playback/home chain and before the masters, so a grand
master still dims it. It is never a programmer write, so a store cannot capture
it, and colour and position are untouched. A blackout clears it, blind
suppresses it, and `Clear` (selection gone) and `Release` clear it too.

## 16-bit attributes and channel sets (08)

A profile's fine DMX slot is no longer dropped on import: `Channel.fine_offset`
records the low byte, the mode's footprint covers it, and the renderer writes
both bytes from one interpolated 16-bit value. A 50% pan is `0x7F 0xFF`; an
8-bit profile keeps its existing byte rounding and expands the byte by 257, so
its output does not move. `raw_from_level` is the high byte of
`raw16_from_level`, and `level_from_raw16` is the inverse.

```
Fixture 1 At Pan 50                 one level, two DMX bytes
Fixture 1 Gobo 2                   the second set, by 1-based index
Fixture 1 Gobo "Rotate CW"         by exact, case-insensitive name
Fixture 1 Gobo                     list bands and the current set
Group 1 Gobo "Open"                every member that has the attribute
```

A channel set is the profile's existing `ChannelFunction`, so no new show data
was needed for it. Choosing one stores the midpoint of its physical band as an
ordinary level, which fades and stores like any other value; the listing prints
both the physical band and the DMX bytes, including the fine byte on a 16-bit
channel. A mode that uses one slot twice — coarse or fine — is rejected at load
with both attributes named. Show v10.

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
- **Only ten of the eighteen roadmap designs are built.** The remaining
  eight are listed in [`ROADMAP.md`](ROADMAP.md).

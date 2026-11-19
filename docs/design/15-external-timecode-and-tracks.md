# 15 · External timecode and tracks

**Rank** 15 · **Order** 13 · **Size** L · **Show** v12 · **Depends on** 12 (OSC source), 14 (effect actions), 01 (executors)

## Problem

The timecode player is an internal clock. It follows nothing: no MIDI Time Code
from a playback rig, no timecode from a show-control system, no second timeline
for effects or macros. One track of cue events is also too little: a song needs
cues on one list, effect triggers on another, and markers both for the operator
and for whoever is running sound.

## Behaviour when this is done

```
Store Timecode 1 "Song" Fps 25 Source Mtc "/dev/ttyACM0"
Timecode 1 Track 1 "Cues"
Timecode 1 Track 2 "FX"
Timecode 1 Track 1 Add 00:01:00:00 Sequence 1 Cue 5
Timecode 1 Track 2 Add 00:01:04:00 Effect 3 On Group 2
Timecode 1 Marker 00:02:00:00 "Chorus"
Timecode 1 Add 00:01:10:00 Sequence 1 Cue 6     (track 1 is the default)
```

```text
Timecode 1 "Song" — 25 fps, source Mtc "/dev/ttyACM0", 2 tracks, 3 events
Track 1 "Cues"   3 events
  00:01:00:00  Sequence 1 cue 5
  00:01:10:00  Sequence 1 cue 6
Track 2 "FX"     1 event
  00:01:04:00  Effect 3 on Group 2
Markers: 00:02:00:00 Chorus
```

With MTC running, the playhead follows the external clock; without it, the
internal clock drives the same track.

## Design

### Sources

```rust
pub enum TimecodeSource { Internal, Mtc { port: String }, Osc }

pub struct TimecodeShow {
    // … existing …
    pub source: TimecodeSource,   // default Internal
    pub tracks: Vec<Track>,
}

pub struct Track { pub number: u8, pub name: String, pub events: Vec<Event> }
pub struct Event { pub at: String, pub action: Action }
pub enum Action { Cue { sequence: u16, cue: f64 }, Effect { effect: u16, fixtures: Option<Vec<u16>>, on: bool } }
```

- **Internal**: today's behaviour, unchanged.
- **Mtc**: MIDI Time Code quarter-frame messages on a serial port at 31 250
  baud, 8N1 (`crates/dmx` already has the serial plumbing; the MTC reader is a
  separate small `crates/engine/src/mtc.rs` that owns its own thread and posts
  positions into a `mpsc` channel the engine drains on tick). Decoded: hour/
  minute/second/frame plus the frame-rate bits (24, 25, 30 drop, 30 non-drop).
  Full-frame (`0xF0 0x7F ... 0x01 0x01`) and quarter-frame (`0xF1`) are both
  accepted; a `0xF2` (song position) is ignored with a warning.
- **Osc**: an OSC address `/openlight/timecode` with an `f`/`s` position, routed
  from item 12's listener. This is the cheapest way to sync to a system that
  already speaks OSC and has no MIDI.

**LTC is deliberately out of scope.** Decoding linear timecode needs an
audio-rate capture path (biphase mark decoding, adaptive thresholding, a
sound-card or ADC backend) which is a project of its own and shares nothing with
this transport layer. The docs say "MTC or OSC, not LTC", and the source enum is
`#[non_exhaustive]`-style open so LTC can be added without a format break.

### Following external time

The rule that makes a slave playhead usable:

| Incoming position vs expected | Behaviour |
|---|---|
| within ±2 frames | treat as the clock: advance, fire events in between |
| further away | **locate**: jump, fire nothing |

So a jog, a loop or a seek never machine-guns a cue list, and normal playback
(with its sub-frame jitter) still fires events on the frame they are due. At
30 Hz tick and 25 fps a ±2-frame window is ~80 ms, which absorbs MTC's
quarter-frame arrival quantisation without absorbing a real jump.

- Timecode `Start`/`Pause`/`Stop` still work: with an external source, `Start`
  arms the follower (it ignores positions until one arrives within the window of
  the last known position or the first position after arming is taken as a
  locate), `Pause` freezes, `Stop` disarms.
- Loss of external signal for > 500 ms marks the timecode `stalled` in `Status`
  and keeps the playhead where it was; > 5 s stops it, so a cue list does not
  resume from a stale position after a cable is reconnected.
- Drop-frame is handled in `mtc.rs` (`29.97` frames of 30 with two dropped frame
  numbers per minute except every tenth) with a round-trip test; the internal
  player stays integer-framed and is told the frame rate, not the drop rule.

### Tracks

- `Track.number` is 1-based and unique; `Track 1` is the default for
  `Timecode 1 Add …`, so every existing script and the Phase 4 demo keep
  working unchanged. The v11 reader wraps today's flat `events` into track 1.
- Each track's events are sorted and unique by position **within the track**;
  two tracks may fire on the same frame, and they fire in track order.
- Actions are generalised from "start a cue" to a small enum: cues (today) and
  effects on/off (item 14). Item 16 adds `Macro <n>` to the same enum, an
  additive change to the field. Each action goes through the same `Engine`
  methods the command line uses, not a second implementation.
- Markers are per timecode show (not per track) with `at` and `name`; they are
  listed and shown in the shell, and have no behaviour.

### Validation

- positions parse at the show's fps and are inside 0..24 h;
- events unique and ascending per track, tracks unique and non-empty of events
  allowed;
- cue actions target an existing sequence and cue (as today); effect actions
  target an existing effect (and a macro action, once item 16 adds it, an
  existing macro);
- source: `Mtc` needs a quoted, non-empty port; validation does not open it (a
  show must load on a machine without the hardware). At runtime a failed open is
  reported in `Status` as `source unavailable` and the playhead **stops**
  following, because silently running an internal clock while the operator
  expects external sync would put the cue list out of time.

### Commands

| Command | Effect |
|---|---|
| `Store Timecode 1 Source Mtc "/dev/ttyACM0"` | set the source (also `Internal`, `Osc`) |
| `Timecode 1 Track 2 "FX"` | create/rename a track |
| `Timecode 1 Track 2 Add 00:01:04:00 Effect 3 On Group 2` | add an action |
| `Timecode 1 Track 2 Remove 00:01:04:00` | remove |
| `Timecode 1 Track 2` | list one track |
| `Timecode 1 Marker 00:02:00:00 "Chorus"` | add a marker |
| `Timecode 1 Add 00:01:10:00 Sequence 1 Cue 6` | unchanged, track 1 |

## Implementation plan

1. `Track`, `Event`, `Action`, `TimecodeSource`, v11→v12 upgrade (flat events →
   track 1), validation and listing.
2. Generalise `fire_timecode_event` into `apply_action`, keeping cues on the
   exact path they use now.
3. `mtc.rs`: quarter-frame assembly, drop-frame maths, reader thread with a
   bounded channel, and tests driven by synthetic bytes (no hardware).
4. Follower logic in `Engine::tick`: window compare, locate vs advance, stall
   and stop timeouts, `Status` fields.
5. OSC source hook from item 12's mapping.
6. Extend the Phase 4 demo's timecode section with a second track and a marker.

## Tests

- **timecode** — track events are sorted, unique and independent; the same frame
  in two tracks fires both in track order; track 1 default keeps old scripts
  working; markers list.
- **mtc** — quarter frames reassemble a position; a full-frame message is
  accepted; 29.97 drop-frame round-trips across a minute boundary; a torn
  message is ignored; a wrong frame-rate bit is reported.
- **engine** — advancing within ±2 frames fires due events exactly once;
  a 10-frame jump locates and fires nothing; a 300 ms signal loss marks stalled
  and a 6 s loss stops; an effect action turns the effect on for the group's
  fixtures; a macro action runs the macro; a missing MTC port is reported and
  does not fall back to internal.
- **show** — v11 upgrade wraps events into track 1; an event on a missing track
  or targeting a missing effect is rejected with the reason; source round-trips.
- **command** — every new form parses; `Source Ltc` is rejected with the list of
  supported sources.

## Boundaries

- No LTC, no SMPTE over audio, no timecode output (the desk follows, it does not
  generate).
- No tempo map inside the timecode show; item 14's desk tempo is separate and
  not automated by tracks.
- No track automation curves, no per-track offsets, no chasing at 2× rate.
- No marker-triggered actions (markers are labels).

## Risks

- **Serial contention.** An MTC port and a DMX port on the same USB-serial chip
  are common; the design opens its own path and reports a failure rather than
  silently sharing. The docs list known-good adapters.
- **Frame-rate mismatch.** A show at 25 fps following 30 fps MTC would run 20%
  fast. The follower rejects a rate mismatch at arm time with a message naming
  both rates, rather than resampling.
- **Two clocks.** Internal and external never run together: the source decides,
  and `Start` under an external source arms rather than starts. The state
  machine has one owner (`TimecodePlayer`) and one mode field, which is why the
  tests can enumerate it.

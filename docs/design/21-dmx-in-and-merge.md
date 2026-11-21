# 21 · DMX in and merge

**Rank** 4 (follow-up) · **Order** 21 · **Size** L · **Show v14, desk v1** · **Depends on** 01 (output runtime), 02 (patch), 04 (journal)

## Problem

The output path is write-only. A desk can drive a rig, but it cannot see a
second console on the same cable, cannot merge a house console with its own
look, and cannot react to a DMX switch or foot pedal. Every venue with a
backup desk, a house system or a DMX-triggered effect still needs a second
piece of hardware to do the merge, and a show cannot use an incoming universe
as an input at all.

The missing read path also blocks the rest of the DMX chapter: RDM, DMX
triggers and a tracking backup all need to receive a universe before they can
do anything with it.

## Behaviour when this is done

```
Dmx                                  list every input and its merge
Dmx In                               the same listing
Dmx 1                                one universe's input, merge and meter
Dmx 1 Merge Htp                      merge local and input per slot, highest wins
Dmx 1 Merge Ltp                      merge per slot, most recently changed wins
Dmx 1 Merge Off                      stop merging; keep reading and metering
Dmx 1 Priority Local                 local wins an LTP tie
Dmx 1 Priority Input                 input wins an LTP tie
Dmx 1 Timeout 1.5                    signal is stale after 1.5 s
Dmx 1 Loss Hold                      keep the last input after it goes stale
Dmx 1 Loss Zero                      drop the input's contribution at once
Dmx 1 Loss Fade 2                    fade the input out over 2 s, then drop it
Dmx 1 Trigger 10 Sequence 2          slot 10 at or above 128 fires `Go Sequence 2`
Dmx 1 Trigger 10 Sequence 2 Threshold 200
Dmx 1 Trigger Off                    remove the trigger
Dmx 1 Snapshot                       copy the incoming raw look into the programmer
```

```text
> Dmx In
DMX in (1 universe)
Universe 1  virtual                  ok           frames 421  peak 255  last 12 ms ago  discarded 0  errors 0
            merge HTP  priority Local  loss Hold  timeout 1000 ms  trigger off

> Dmx 1 Merge Ltp
Universe 1 merges input with LTP

> Dmx 1
Universe 1  virtual                  ok           frames 421  peak 255  last 12 ms ago  discarded 0  errors 0
            merge LTP  priority Local  loss Hold  timeout 1000 ms  trigger off

> Dmx 1 Snapshot
Snapshotted 17 values from universe 1 into the programmer
```

- An input is read continuously even while its merge is `Off`. The status and
  meter keep working, so an operator can check a feed before enabling it.
- HTP merges slot by slot: the larger of the local value and the input value
  is sent. It is order-independent and is the safe default for a backup desk.
- LTP merges slot by slot: the source whose value most recently changed owns
  that slot. A local cue moving one channel does not wipe the other 511 input
  channels. When both sources changed in the same engine frame, source
  priority decides the tie.
- Local output is never suppressed by an input that has never produced a
  frame. Loss of signal affects only the input's contribution; the desk's own
  look keeps running.
- The local masters and blackout apply to the local contribution only, not to
  a merged input. `Blackout` therefore cannot silence a house console that is
  feeding the same universe; `Dmx <n> Merge Off` is the explicit way to stop
  merging it.
- A stale input follows the loss policy: `Hold` keeps its last look, `Zero`
  removes it immediately, and `Fade` scales it to zero over the configured
  time before removing it. While a fade is running the engine keeps rendering,
  so the output follows the fade rather than freezing at its first step.
- A start code other than zero is not a dimmer frame. It is discarded, counted
  and reported; it never reaches the merge.
- The trigger is a rising edge on one raw input slot. It fires `Go Sequence
  <n>` through the ordinary playback path, once per crossing; the slot must
  fall below the threshold before it can fire again. The first frame above a
  newly configured threshold is the baseline, not a rising edge: the slot has
  to go low once before it can trigger. A trigger works with the merge off.
- `Dmx 1 Snapshot` reads the last raw input frame into the programmer through
  the patched profiles, including 16-bit channels when the fixture has a fine
  channel. It snapshots the incoming look, not the merged output, and it works
  while the input is stale because the last received frame is still the look
  the other console sent.
- All input configuration lives in the show and is saved, undoable and
  journaled. Reading frames, counters and the meter are session state and are
  never saved.

## Design

### Show v14

```rust
pub enum MergeMode {
    Off,
    Htp,
    Ltp,
}

pub enum InputPriority {
    Local,
    Input,
}

pub enum LossPolicy {
    Hold,
    Zero,
    Fade(f64),
}

pub struct InputTrigger {
    /// 1-based DMX address in the incoming frame.
    pub address: u16,
    /// Fire when the slot reaches this value (1..=255).
    pub threshold: u8,
    /// Sequence to `Go` on the rising edge.
    pub sequence: u16,
}

pub struct Universe {
    pub number: u16,
    pub label: String,
    pub size: u16,
    #[serde(default)]
    pub merge: MergeMode,
    #[serde(default)]
    pub priority: InputPriority,
    #[serde(default)]
    pub loss: LossPolicy,
    #[serde(default = "default_input_timeout_ms")]
    pub input_timeout_ms: u64,
    #[serde(default)]
    pub input_trigger: Option<InputTrigger>,
}
```

- Every new field has a serde default, so a v13 document loads with merging
  off, local priority, hold-on-loss and a one-second timeout. `SHOW_VERSION`
  moves to 14 because the document shape changed; `READABLE_VERSIONS` adds 14.
- The configuration is per universe, not per input binding. An input bound to
  a universe inherits that universe's settings, and a show moved to another
  machine keeps its merge policy.
- Validation adds: timeout `1..=60000` ms, a finite fade in `0..=60` s,
  trigger address `1..=512`, threshold `1..=255`, and a trigger sequence that
  exists. `Dmx 1 Trigger ...` rejects a missing sequence instead of storing a
  trigger that can never fire.
- `Universe` gets a `Default` implementation so the handful of literal
  constructors in imports, tests and the renderer stay short.

### Input transports

```rust
pub struct DmxFrame {
    pub start_code: u8,
    pub slots: [u8; DMX_SLOTS],
}

pub trait DmxInput: Send {
    fn describe(&self) -> String;
    fn poll(&mut self, timeout: Duration) -> DmxResult<Option<DmxFrame>>;
    fn take_warning(&mut self) -> Option<String>;
    fn reopen(&mut self) -> DmxResult<()>;
}
```

- `virtual` is the hardware-free reader. It is an mpsc channel with a handle;
  a test, a demo or a future simulator sends whole frames and `poll` returns
  them in order. Its `reopen` succeeds, so the reconnect path is testable.
- `enttec:<port>` is a real reader for the DMX USB Pro protocol. It configures
  the port for 57600 8N1, sends label 8 with payload `0` ("send always"), and
  parses unsolicited label 5 messages: status byte, start code and up to 512
  slots. A short payload is padded with zeros; a non-zero status is dropped
  with a warning. The parser resynchronises on the next `0x7E` after a bad
  length or missing `0xE7`, so one corrupt USB packet does not poison the
  stream.
- `generic:` input is deliberately unsupported in this item. A bare serial
  DMX receiver needs break detection before it can find a frame boundary, and
  the platform serial layer has only been proven for output. Reporting
  `unsupported` is safer than guessing at framing from slot values. The
  hardware pass owns that adapter.
- `dmx::open_input` accepts `virtual` and `enttec:<port>`; unknown specs fail
  with the same style as `dmx::open`. Physical protocols are gated by the
  hardware proof, not by a silent fallback to virtual.

### Runtime and threads

```rust
pub struct InputBinding {
    pub universe: u16,
    pub input: Box<dyn DmxInput>,
}

impl Runtime {
    pub fn start(...) -> Self;
    pub fn start_with_inputs(
        engine: Engine,
        transports: Vec<TransportBinding>,
        inputs: Vec<InputBinding>,
        config: RuntimeConfig,
    ) -> Self;
}
```

- Each input gets one reader thread, just as each output gets one worker. A
  blocked or failed reader only stalls its own thread; the engine tick and
  every output worker keep running. `poll` has a finite timeout so shutdown
  waits at most one poll interval.
- The reader thread locks the engine only to deliver one frame and update
  counters. It never holds the lock while reading the device.
- A read error records the error and puts that input into `reconnecting`; the
  thread retries `reopen` with the same exponential backoff as output, up to
  8 s, and returns to `ok` on the next good frame. A corrupt ENTTEC packet is
  a warning and a dropped frame, not a reconnect.
- `Runtime::start` keeps its old signature and delegates to
  `start_with_inputs` with no inputs, so every existing caller and test is
  unchanged.

### Merge state

`engine::input::InputMixer` owns one universe's session state:

```rust
pub struct InputMixer {
    last: Option<[u8; DMX_SLOTS]>,
    received_at: Option<Instant>,
    local_previous: Option<[u8; DMX_SLOTS]>,
    local_changed: [bool; DMX_SLOTS],
    input_changed: [bool; DMX_SLOTS],
    owner: [Owner; DMX_SLOTS],
    trigger_armed: bool,
    loss_rendered: bool,
}
```

- **Change flags.** `receive` marks the slots that differ from the previous
  input frame; `merge` marks the slots that differ from the previous local
  frame. Flags are per slot, not per universe, which is what makes LTP usable
  with a desk that is also running a cue. A merge consumes them, so each
  change is arbitrated once.
- **HTP.** For every slot, `max(local, effective_input)`. Ties are identical
  values, so priority has no visible effect; the listing still shows it
  because it applies to LTP.
- **LTP.** A slot with a current input contribution uses the input value when
  only the input changed, keeps the local value when only the local changed,
  and falls back to the remembered owner when neither changed. When both
  changed in the same engine frame, priority is `Local` or `Input`. When the
  input is stale and its loss policy has removed it, local owns every slot
  regardless of the old owner.
- **Display is read-only.** A window asks for `Engine::display_frame`, which
  merges a clone of the mixer state. Only the output thread calls `render`, so
  a repaint cannot consume an LTP change or the final step of a loss fade.
- **Loss.** A frame is fresh until `received_at + input_timeout_ms`. After
  that, `Hold` keeps the last values, `Zero` removes the contribution, and
  `Fade(s)` scales it by `1 - (age - timeout) / s`, clamped at zero. Fade
  state is computed from the engine clock at render time, so a slow engine
  still produces the correct ramp.
- **Fade keeps rendering.** `Engine::tick` reports a change while any merged
  input is fading, exactly as it does for a running effect. Without that, the
  sender would stop at the first stale frame because no revision changed.
- **Missing show universe.** An input may be bound to a universe the show does
  not have. It is read and metered, but it cannot merge; `Dmx` marks it
  `unmapped` instead of writing to a universe nobody renders.
- **Start code.** `DmxFrame.start_code != 0` is counted as discarded and never
  replaces `last`. The virtual reader can send an alternate start code to
  test this; the ENTTEC reader passes the byte through.

### Engine integration

- `Engine` gains a session-only `BTreeMap<u16, InputMixer>` and
  `apply_input(universe, frame, now)`. `apply_input` updates the input status,
  stores the frame and marks its changed slots, evaluates the trigger and
  bumps the revision when the universe actually merges or a trigger starts a
  sequence. The trigger calls the same `playback_go` path as the command, so
  cue timing, follow triggers and the message are identical.
- `Engine::render` takes `&mut self` and applies each mixer to the rendered
  frame after masters, blackout and highlight. Input is therefore the last
  stage before the wire, which is the only place a second console can belong:
  it must not alter the programmer, a cue, a palette or a snapshot. The
  preview frame deliberately does not see input.
- The output engine thread owns `render`; `App::snapshot` uses
  `display_frame`, which merges a clone of the mixer state, so the GUI meter
  and the wire show the same values without the window consuming an input
  edge. The sheet and preview remain local-only views because they are
  programming tools, not stage monitors.
- A `Dmx` command that changes merge policy is a normal show edit through
  `mutate`, so it is validated, journaled, autosaved and undoable. Read
  counters and the meter are not.

### Commands and listings

| Command | Effect |
|---|---|
| `Dmx`, `Dmx In` | list every bound input and every universe's merge |
| `Dmx <n>` | one universe's input, merge, meter and trigger |
| `Dmx <n> Merge Off\|Htp\|Ltp` | set the merge mode |
| `Dmx <n> Priority Local\|Input` | set the LTP tie-break |
| `Dmx <n> Loss Hold\|Zero\|Fade <s>` | set the loss policy |
| `Dmx <n> Timeout <s>` | set the stale timeout in seconds |
| `Dmx <n> Trigger <slot> Sequence <n> [Threshold <v>]` | store a rising-edge trigger |
| `Dmx <n> Trigger Off` | remove the trigger |
| `Dmx <n> Snapshot` | copy the raw input frame into the programmer |

- `Dmx` is a new command word; `In`, `Merge`, `Priority`, `Loss`, `Hold`,
  `Zero`, `Fade`, `Timeout`, `Trigger`, `Threshold`, `Snapshot` are reserved
  where the parser reads them. None is a current fixture attribute name.
- `Dmx <n>` on a universe that is not in the show is an error with a hint,
  because merge settings belong to a universe record. The input listing still
  shows a bound universe that the show does not contain, marked `unmapped`.
- The listing shows transport, signal state, frames, peak, age, merge,
  priority, loss, timeout and trigger. `Status` adds one line per input after
  the output universes. The remote `/state` JSON gains an `inputs` array with
  universe, transport, state, frames, peak, age, errors, reconnects, merge,
  priority, loss and trigger.
- `Dmx <n> Snapshot` is a programmer edit. It writes every patched channel in
  that universe, respecting the fixture's profile mapping and 16-bit fine
  channels, and reports how many values it captured. It uses the last frame
  even when stale; with no frame at all it refuses.

## Implementation plan

1. `dmx`: `DmxFrame`, `DmxInput`, `virtual_input`, `enttec_input`, and
   `open_input`; tests for virtual delivery, ENTTEC framing/handshake/resync
   and unsupported generic input.
2. `show.rs`: merge types, trigger type, `Universe` fields and default,
   validation, `SHOW_VERSION` 14 and the v13 upgrade.
3. `engine/input.rs`: `InputMixer`, change stamps, HTP/LTP, loss/fade and
   trigger edge; unit tests with a fixed clock.
4. `engine/output.rs`: `InputBinding`, `start_with_inputs`, reader thread,
   input status initialisation, reconnect/backoff and shutdown.
5. `engine/engine.rs`: mixer map, `apply_input`, merged `render`, fade-aware
   `tick`, status lines, remote JSON, snapshot capture and dispatch.
6. `command.rs`: `Dmx` parser, variants, `mutates` classification and tests;
   help text.
7. `app` and `console`: `AppOptions.inputs`, `--input`, startup wiring,
   virtual handles and status output.
8. Docs: this design, roadmap row and format ladder, `PHASE5.md`, unreleased
   `CHANGELOG.md`, and regenerated `COMPLETENESS.md`/matrix.

## Tests

- **dmx** — a virtual frame round-trips with its start code; a closed handle
  ends `poll`; an ENTTEC label-5 message decodes status, start code and slots;
  a short payload pads; a corrupt status is dropped with a warning; a bad
  length and a missing end delimiter resynchronise on the next frame; the
  first poll configures 57600 8N1 and writes label 8 with payload `0`;
  `open_input("generic:...")` reports unsupported and an unknown spec names
  the supported readers.
- **show** — a v13 document loads with merge off and saves as v14; a v14
  document round-trips every merge field; an invalid timeout, fade, trigger
  address, threshold or missing trigger sequence is rejected; the default
  `Universe` is valid.
- **input mixer** — HTP takes the max per slot; LTP follows per-slot changes
  and does not let a one-slot local change take the universe; ties follow
  priority; `Hold`, `Zero` and `Fade` remove the contribution at the expected
  times; a fade produces intermediate values and stops changing after it
  finishes; a non-zero start code is never stored; a trigger fires once on a
  rising edge and re-arms below the threshold.
- **engine** — `Dmx 1 Merge Htp` changes the show and is undoable; an input
  frame reaches `render` through HTP and LTP; loss policy changes the rendered
  frame; a trigger fires `Go Sequence`; `Dmx 1 Snapshot` captures patched
  values, including a fine channel, and refuses with no frame; `Dmx`, `Dmx In`
  and `Dmx 1` list the expected fields; `Status` and the remote JSON include
  inputs; `preview_frame` and the sheet stay local.
- **runtime** — a fake input reader delivers a frame to the engine and the
  output worker sends the merged value; a reader that blocks/errors does not
  stop output; a failing reader reconnects and resumes; stopping the runtime
  joins the input thread.
- **app/console** — `--input 1=virtual` starts an input thread and reports it;
  a bad input spec fails startup; `--status` does not open an input.

## Boundaries

- **No RDM.** RDM needs a request/response transaction layer on top of the
  read path; this item only consumes start-code-zero dimmer frames.
- **No Art-Net or sACN.** Network protocols are a separate transport family,
  with their own discovery, priority and sequence handling.
- **No full tracking backup.** A backup desk needs a second show engine,
  state synchronisation and a deterministic takeover, not just a merge.
- **No generic serial input.** Break detection is not proven on real
  adapters; `generic:` input reports unsupported until the hardware pass.
- **No per-attribute merge.** HTP and LTP apply to every slot. Intensity-only
  HTP, colour LTP and per-attribute priorities are a later mixing model.
- **No input-to-programmer live tracking.** `Snapshot` is a one-shot capture;
  a continuous "follow the input" mode would fight the programmer and needs
  its own ownership rules.
- **No GUI input pane or visual meter.** The status listing and remote JSON
  carry the meter; a dedicated pane is GUI depth, not the read path.
- **No physical verification here.** The ENTTEC reader is tested against a
  fake serial link, not a widget. The hardware matrix and adapter proof remain
  the release gate.

## Risks

- **A merge can put a second console on stage.** Enabling HTP on a live
  universe immediately adds whatever the input is sending. The listing and
  meter make the feed visible before the merge is enabled, `Merge Off` is the
  default, and the change is undoable; still, the operator must check the
  feed first.
- **Blackout does not cover a merged input.** The desk's blackout, grand master
  and group masters act on the local look before the input stage. That is the
  standard merge order, but it means an operator who expects blackout to kill
  everything must use `Dmx <n> Merge Off` (or the other console) as well. The
  input listing and state make it clear that a foreign source is contributing.
- **LTP is inherently ambiguous when both sources change together.** The
  priority tie-break makes the result deterministic and visible in the
  listing, but it cannot know which console the operator meant. HTP is the
  safer default for backup use.
- **Hold-on-loss can freeze an old look.** Holding a dead console's last
  frame is useful for a controlled takeover and dangerous for an unattended
  rig. The state is reported as stale and the fade/zero policies are one
  command away; the default is documented rather than hidden.
- **A stale frame has no wall-clock timestamp after a restart.** Timestamps
  are monotonic and session-only; a restarted process has no input until the
  first frame, which is exactly the no-signal state. No stale value is
  restored from disk.
- **The ENTTEC reader is protocol-complete but hardware-unproven.** Label 8's
  payload and label 5's framing come from API 1.44, and the parser is tested
  with fake links, but a widget in receive mode has not been observed here.
  The hardware pass must confirm it before the row is marked verified.
- **Input threads can outlive a wedged driver.** `poll` is given a finite
  timeout and shutdown joins with the same two-second bound as output, but a
  driver that ignores the timeout can delay exit. The input is best-effort
  and cannot block the output workers.

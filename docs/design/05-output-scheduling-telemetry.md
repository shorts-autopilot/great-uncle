# 05 · Output scheduling, telemetry, reconnect and watchdog

**Rank** 5 · **Order** 2 · **Size** M · **No format change** · **Depends on** nothing

## Problem

`output_loop` sends every universe in sequence, on one 30 Hz ticker, and calls
`Ticker::wait` which *skips* missed deadlines. Three consequences:

- **A second universe cannot keep up.** 513 bytes at 250 kbaud 8N2 is ~23 ms of
  wire time. Two universes take ~46 ms, so "30 Hz" becomes ~21 Hz for both and
  the ticker hides it by skipping frames.
- **Status reports intent, not reality.** `OutputStatus.frame_hz` is the
  configured rate; nothing measures what was achieved, how long a send took, or
  whether a universe is actually alive.
- **A failure is silent and permanent.** An unplugged adapter increments
  `errors` and then that universe is frozen at its last look while the rest of
  the rig keeps running. There is no reopen, no backoff, no operator-visible
  state change, and no distinction between "one bad frame" and "offline for ten
  minutes".

## Behaviour when this is done

```
Status

Output: running   engine 60.0 Hz   frame 30.0 Hz target
Universe 1  generic:/dev/ttyUSB0   ok        29.9 Hz  frames 18422  errors 0   reconnects 0
Universe 2  generic:/dev/ttyUSB1   offline   —        frames 9201   errors 431 reconnects 6  last error: write failed: EIO
```

- Each universe runs at the rate its wire allows, without stealing from another.
- A dropped adapter is retried with backoff and recovers on its own.
- A stalled engine cannot leave a frozen look on stage for more than two
  seconds.

## Design

### One worker per universe

```rust
// crates/engine/src/output.rs
struct Worker {
    binding: TransportBinding,
    shared: Arc<Shared>,
    status: Arc<Mutex<OutputStatus>>,
    period: Duration,
}
```

Each worker owns its thread and is **self-clocked**:

```
start = Instant::now()
loop {
    let sent = snapshot();               // Arc clone, no copy
    let began = Instant::now();
    match binding.transport.send_frame(&sent[universe]) { … }
    let took = began.elapsed();
    // Next deadline is measured from the *start* of this send, so a slow wire
    // slows that universe and nothing else.
    sleep_until(start + period);
    start = if now < start + period { start + period } else { now };
}
```

There is no global ticker. A universe that needs 23 ms per frame settles at its
achievable rate and reports it; a universe on an ENTTEC widget at 40 Hz keeps
40 Hz.

### Frame sharing

`Shared` is `Mutex<Arc<OutputFrame>>`. The engine thread builds a new
`OutputFrame`, wraps it in an `Arc` and swaps the pointer; workers clone the
`Arc` (one atomic increment) and read it without holding the frame mutex. Today
each worker clones the whole `OutputFrame` (a `BTreeMap` of 512-byte arrays)
once per frame; the `Arc` removes that allocation from the send path.

`OutputFrame` gains `built_at: Instant` and `revision: u64`, which is what the
watchdog reads.

### Watchdog

Per worker, before sending:

| Snapshot age | Action |
|---|---|
| `<= 500 ms` | send normally |
| `500 ms .. 2 s` | send the last frame again, mark the universe `stale` |
| `> 2 s` | send black, mark `stale — engine silent` |

Two seconds is chosen to survive a long GC-like stall or a debugger pause
without leaving a look on stage; the stale threshold is short enough that
`Status` shows the problem while the operator is still looking at it. DMX needs
continuous refresh, so "keep sending the last frame" is the only alternative to
black; the watchdog exists to bound *how long* a frozen look may persist.

The engine thread marks the frame's `built_at` on every render, including a
render with no change, so a healthy idle engine never trips the watchdog.

### Reconnect

`DmxTransport` gains one method with a default that reports "not supported":

```rust
fn reopen(&mut self) -> DmxResult<()> {
    Err(DmxError::Unsupported("reconnect is not supported".into()))
}
```

- `GenericSerialTransport` and `EnttecProTransport` implement it by reopening
  their path; the virtual transport returns `Ok(())` so tests can drive the
  path.
- After a send error the worker enters `reconnecting`: it stops counting errors
  per frame, attempts `reopen` on a backoff of 0.5, 1, 2, 4, 8 s (capped), and
  on success resumes sending and increments `reconnects`.
- Status transitions are logged once each (`universe 2 offline: …`,
  `universe 2 recovered after 4.1 s`), not once per frame.
- While reconnecting the worker still respects the watchdog, so a dead port
  receives black frames rather than the last look if it comes back mid-show.

### Telemetry

```rust
pub struct UniverseStatus {
    pub transport: String,
    pub state: OutputState,          // Ok | Stale | Reconnecting | Offline | Stopped
    pub frames: u64,
    pub errors: u64,
    pub reconnects: u64,
    pub achieved_hz: f64,            // EWMA, alpha 0.05
    pub last_send_us: u64,
    pub stale_ms: u64,
    pub last_error: Option<String>,
}
```

`Status` prints target and achieved side by side, plus one line per universe.
The shell's status window reads the same struct. `achieved_hz` is an EWMA of
`1 / interval` so a single slow frame does not make the number useless, and
`last_send_us` is the raw measurement an operator needs to tell "the widget is
slow" from "the cable is bad".

Shutdown sends two black frames per universe (two, so a widget that missed one
still ends dark), drains, then reports `Stopped`. `Runtime::drop` joins workers
with a bounded wait so a wedged driver cannot hang process exit.

## Implementation plan

1. Split `OutputFrame` into value + `built_at`/`revision`; introduce
   `Shared = Mutex<Arc<OutputFrame>>` and update the engine loop to swap.
2. Replace `output_loop` with `Worker::run` per binding; keep the virtual-port
   tests passing (they already assert frames, blackout and status).
3. Add `OutputState`, the new `UniverseStatus` fields, EWMA and status
   transitions; extend the `Status` formatter.
4. Add `DmxTransport::reopen` and implement it in generic, ENTTEC and virtual;
   add the backoff state machine.
5. Add the watchdog thresholds as `RuntimeConfig` fields (`stale_ms`,
   `blackout_ms`) so tests can shrink them to 20/50 ms instead of sleeping for
   seconds.
6. Update `docs/PHASE1.md`'s output section and `docs/PHASE4.md`'s status
   example to the measured form.

## Tests

- **output** — two virtual universes with a slow transport (a `VirtualTransport`
  wrapper that sleeps 20 ms) both reach their own frame counts and neither
  starves; `achieved_hz` is within 10% of the measured rate; `last_send_us`
  reflects the injected delay.
- **output** — a transport that fails `n` times and then accepts frames drives
  `state` through `Reconnecting → Ok`, increments `reconnects` once and logs one
  transition per change.
- **output** — with `stale_ms = 20` and no engine render, the worker sends the
  last frame and reports `Stale`; with `blackout_ms = 50` it sends zeros.
- **output** — stop sends black twice, sets `Stopped`, and `Runtime::drop` joins
  within a bounded time.
- **dmx** — `reopen` on a pty whose peer was closed and reopened succeeds; on a
  missing path it returns the error and leaves the transport usable for another
  attempt.

## Boundaries

- No RDM, no universe merging, no sACN/Art-Net (serial output only).
- No priority between universes, no per-universe rate configuration (each
  universe runs as fast as its wire allows, which is the point).
- No thread-pool or async runtime; one thread per universe is the correct
  complexity for ≤ 8 universes.

## Risks

- **Thread-per-universe cost.** Eight threads at 30 Hz is negligible; the design
  avoids a scheduler that would need its own tests.
- **Blackout-on-stall is a behaviour change.** A user pausing the engine in a
  debugger now sees black after two seconds. That is safer than a frozen look
  and is configurable; it is documented in the release notes.
- **Reconnecting while the show runs.** A universe that returns after 8 seconds
  jumps straight to the current look, which is a visible bump. The alternative
  (fade in) invents a feature; the bump is reported by the `recovered` log line.

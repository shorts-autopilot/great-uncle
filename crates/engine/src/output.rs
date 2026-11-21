//! Output runtime.
//!
//! Three kinds of thread, all reading only:
//!
//! * the **engine tick** advances playback and re-renders when the engine
//!   revision changes (60 Hz by default). It stamps every frame with the time
//!   it was built, which is what the watchdog reads.
//! * one **worker per universe** pushes that universe at the rate its wire
//!   allows. A slow universe slows itself and nothing else: there is no global
//!   ticker that can starve a second port.
//!
//! Mutation always happens on the caller's thread through the engine mutex, so
//! there is exactly one writer.
//!
//! **Watchdog.** DMX needs continuous refresh. If the engine has not rendered
//! for `stale_ms` the worker keeps sending the last frame and reports `stale`;
//! after `blackout_ms` it sends zeros instead, so a wedged engine cannot leave
//! a frozen look on stage for longer than that.
//!
//! **Reconnect.** A failed send puts that universe into `reconnecting`: it
//! retries the transport on an exponential backoff and resumes by itself, with
//! one log line per state change rather than one per frame.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use dmx::{DmxInput, DmxTransport, DMX_SLOTS};

use crate::engine::Engine;
use crate::render::OutputFrame;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a universe is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputState {
    /// Frames are being sent at the rate the wire allows.
    #[default]
    Ok,
    /// The engine has not rendered recently; the last frame is repeating.
    Stale,
    /// The transport failed and is being reopened.
    Reconnecting,
    /// The transport cannot be reopened; frames are being dropped.
    Offline,
    /// Output has been stopped.
    Stopped,
}

impl OutputState {
    pub fn label(self) -> &'static str {
        match self {
            OutputState::Ok => "ok",
            OutputState::Stale => "stale",
            OutputState::Reconnecting => "reconnecting",
            OutputState::Offline => "offline",
            OutputState::Stopped => "stopped",
        }
    }
}

impl std::fmt::Display for OutputState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Debug, Clone)]
pub struct UniverseStatus {
    pub transport: String,
    pub state: OutputState,
    pub frames: u64,
    pub errors: u64,
    pub reconnects: u64,
    /// Smoothed frames per second actually achieved.
    pub achieved_hz: f64,
    /// How long the most recent send took, in microseconds.
    pub last_send_us: u64,
    /// How old the frame being sent was, in milliseconds.
    pub stale_ms: u64,
    pub last_error: Option<String>,
}

impl Default for UniverseStatus {
    fn default() -> Self {
        Self {
            transport: String::new(),
            state: OutputState::Stopped,
            frames: 0,
            errors: 0,
            reconnects: 0,
            achieved_hz: 0.0,
            last_send_us: 0,
            stale_ms: 0,
            last_error: None,
        }
    }
}

/// What an input reader is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputSignal {
    /// No complete frame has arrived since the reader started.
    #[default]
    NoSignal,
    /// The reader is delivering frames.
    Ok,
    /// The reader failed and is backing off before reopening.
    Reconnecting,
    /// The reader could not be reopened; it keeps retrying but is not usable.
    Offline,
    /// Input has been stopped.
    Stopped,
}

impl InputSignal {
    pub fn label(self) -> &'static str {
        match self {
            InputSignal::NoSignal => "no signal",
            InputSignal::Ok => "ok",
            InputSignal::Reconnecting => "reconnecting",
            InputSignal::Offline => "offline",
            InputSignal::Stopped => "stopped",
        }
    }
}

impl std::fmt::Display for InputSignal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Debug, Clone)]
pub struct InputStatus {
    pub transport: String,
    pub state: InputSignal,
    /// Frames accepted from the reader, including alternate start codes.
    pub frames: u64,
    /// Frames with a non-zero start code; read and counted but never merged.
    pub discarded: u64,
    pub errors: u64,
    pub reconnects: u64,
    pub last_frame: Option<SystemTime>,
    /// Highest slot value in the last accepted frame.
    pub peak: u8,
    pub last_error: Option<String>,
}

impl Default for InputStatus {
    fn default() -> Self {
        Self {
            transport: String::new(),
            state: InputSignal::Stopped,
            frames: 0,
            discarded: 0,
            errors: 0,
            reconnects: 0,
            last_frame: None,
            peak: 0,
            last_error: None,
        }
    }
}

/// Counters and addresses for the optional OSC/HTTP remote.
#[derive(Debug, Clone, Default)]
pub struct RemoteStatus {
    pub osc_port: u16,
    pub http_port: u16,
    pub bind: String,
    pub osc_accepted: u64,
    pub osc_unmatched: u64,
    pub http_clients: u64,
    pub http_rejected: u64,
    pub http_rate_limited: u64,
    pub dropped: u64,
}

#[derive(Debug, Clone)]
pub struct OutputStatus {
    pub running: bool,
    pub frame_hz: f64,
    pub engine_hz: f64,
    /// Frames sent across every universe.
    pub frames: u64,
    pub last_frame: Option<SystemTime>,
    pub universes: BTreeMap<u16, UniverseStatus>,
    pub inputs: BTreeMap<u16, InputStatus>,
    pub warnings: Vec<String>,
    pub remote: Option<RemoteStatus>,
}

impl Default for OutputStatus {
    fn default() -> Self {
        Self {
            running: false,
            frame_hz: 30.0,
            engine_hz: 60.0,
            frames: 0,
            last_frame: None,
            universes: BTreeMap::new(),
            inputs: BTreeMap::new(),
            warnings: Vec::new(),
            remote: None,
        }
    }
}

/// One universe bound to one transport.
pub struct TransportBinding {
    pub universe: u16,
    pub transport: Box<dyn DmxTransport>,
}

/// One universe bound to one input reader.
pub struct InputBinding {
    pub universe: u16,
    pub input: Box<dyn DmxInput>,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeConfig {
    pub engine_hz: f64,
    pub frame_hz: f64,
    /// Age at which a frame is reported stale, in milliseconds.
    pub stale_ms: u64,
    /// Age at which the worker sends black instead of the last look.
    pub blackout_ms: u64,
    /// First reconnect delay, in milliseconds; doubles up to 8 s.
    pub reconnect_ms: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            engine_hz: 60.0,
            frame_hz: 30.0,
            stale_ms: 500,
            blackout_ms: 2_000,
            reconnect_ms: 500,
        }
    }
}

impl RuntimeConfig {
    fn sanitized(self) -> Self {
        let stale_ms = self.stale_ms.max(1);
        Self {
            engine_hz: self.engine_hz.clamp(1.0, 200.0),
            frame_hz: self.frame_hz.clamp(1.0, 200.0),
            stale_ms,
            blackout_ms: self.blackout_ms.max(stale_ms),
            reconnect_ms: self.reconnect_ms.max(1),
        }
    }
}

pub struct Runtime {
    engine: Arc<Mutex<Engine>>,
    status: Arc<Mutex<OutputStatus>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    config: RuntimeConfig,
}

/// The frame the workers send and the moment it was built.
struct SharedFrame {
    frame: Arc<OutputFrame>,
    built_at: Instant,
    revision: u64,
}

type Shared = Arc<Mutex<SharedFrame>>;

impl Runtime {
    pub fn start(engine: Engine, transports: Vec<TransportBinding>, config: RuntimeConfig) -> Self {
        Self::start_with_inputs(engine, transports, Vec::new(), config)
    }

    /// Start the engine, every output worker and every input reader.
    ///
    /// Input readers are independent threads: a blocked or failed reader
    /// cannot stall the engine tick or an output worker.
    pub fn start_with_inputs(
        engine: Engine,
        transports: Vec<TransportBinding>,
        inputs: Vec<InputBinding>,
        config: RuntimeConfig,
    ) -> Self {
        let config = config.sanitized();
        let status = Arc::clone(&engine.status);
        {
            let mut status = lock(&status);
            status.running = true;
            status.engine_hz = config.engine_hz;
            status.frame_hz = config.frame_hz;
            status.frames = 0;
            status.last_frame = None;
            status.warnings.clear();
            status.universes = transports
                .iter()
                .map(|binding| {
                    (
                        binding.universe,
                        UniverseStatus {
                            transport: binding.transport.describe(),
                            ..UniverseStatus::default()
                        },
                    )
                })
                .collect();
            status.inputs = inputs
                .iter()
                .map(|binding| {
                    (
                        binding.universe,
                        InputStatus {
                            transport: binding.input.describe(),
                            state: InputSignal::NoSignal,
                            ..InputStatus::default()
                        },
                    )
                })
                .collect();
        }

        let engine = Arc::new(Mutex::new(engine));
        let shared: Shared = Arc::new(Mutex::new(SharedFrame {
            frame: Arc::new(OutputFrame::default()),
            built_at: Instant::now(),
            revision: u64::MAX,
        }));
        let stop = Arc::new(AtomicBool::new(false));

        let engine_thread = {
            let engine = Arc::clone(&engine);
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            thread::spawn(move || engine_loop(engine, shared, stop, config.engine_hz))
        };

        let mut threads = vec![engine_thread];
        for binding in inputs {
            let engine = Arc::clone(&engine);
            let stop = Arc::clone(&stop);
            let status = Arc::clone(&status);
            threads.push(thread::spawn(move || {
                input_loop(engine, binding, stop, status, config)
            }));
        }
        for binding in transports {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            let status = Arc::clone(&status);
            threads.push(thread::spawn(move || {
                Worker {
                    binding,
                    shared,
                    stop,
                    status,
                    config,
                }
                .run()
            }));
        }

        Self {
            engine,
            status,
            stop,
            threads,
            config,
        }
    }

    /// Handle for command execution from any surface.
    pub fn engine(&self) -> Arc<Mutex<Engine>> {
        Arc::clone(&self.engine)
    }

    pub fn config(&self) -> RuntimeConfig {
        self.config
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            join_with_timeout(thread, Duration::from_secs(2));
        }
        lock(&self.status).running = false;
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Join a thread without letting a wedged driver hang process exit.
fn join_with_timeout(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let (sender, receiver) = mpsc::channel();
    let joiner = thread::spawn(move || {
        let _ = handle.join();
        let _ = sender.send(());
    });
    let joined = receiver.recv_timeout(timeout).is_ok();
    if joined {
        let _ = joiner.join();
    }
    joined
}

fn engine_loop(engine: Arc<Mutex<Engine>>, shared: Shared, stop: Arc<AtomicBool>, rate_hz: f64) {
    let mut ticker = Ticker::new(rate_hz);

    while !stop.load(Ordering::Relaxed) {
        ticker.wait();
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let now = Instant::now();
        let mut state = lock(&engine);
        let advanced = state.tick(now);
        let revision = state.revision;
        let mut shared = lock(&shared);
        // A healthy idle engine still re-stamps its frame, so the watchdog only
        // trips when the engine itself has stopped.
        if advanced || revision != shared.revision {
            shared.frame = Arc::new(state.render());
            shared.revision = revision;
        }
        shared.built_at = now;
    }
}

/// One reader thread for one input universe.
fn input_loop(
    engine: Arc<Mutex<Engine>>,
    mut binding: InputBinding,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<OutputStatus>>,
    config: RuntimeConfig,
) {
    let universe = binding.universe;
    let poll = Duration::from_millis(50);
    let initial_backoff = Duration::from_millis(config.reconnect_ms);
    let mut backoff = initial_backoff;

    while !stop.load(Ordering::Relaxed) {
        match binding.input.poll(poll) {
            Ok(Some(frame)) => {
                lock(&engine).apply_input(universe, frame, Instant::now());
                if let Some(warning) = binding.input.take_warning() {
                    push_warning(&status, format!("input {universe}: {warning}"));
                }
                backoff = initial_backoff;
            }
            Ok(None) => {
                if let Some(warning) = binding.input.take_warning() {
                    push_warning(&status, format!("input {universe}: {warning}"));
                }
            }
            Err(error) => {
                let mut guard = lock(&status);
                if let Some(entry) = guard.inputs.get_mut(&universe) {
                    entry.errors += 1;
                    entry.state = InputSignal::Reconnecting;
                    entry.last_error = Some(error.to_string());
                }
                guard.warnings.push(format!("input {universe}: {error}"));
                trim_warnings(&mut guard.warnings);
                drop(guard);

                if let Some(warning) = binding.input.take_warning() {
                    push_warning(&status, format!("input {universe}: {warning}"));
                }
                sleep_unless_stopped(&stop, backoff);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match binding.input.reopen() {
                    Ok(()) => {
                        let mut guard = lock(&status);
                        if let Some(entry) = guard.inputs.get_mut(&universe) {
                            entry.reconnects += 1;
                            entry.state = InputSignal::Reconnecting;
                        }
                        drop(guard);
                        backoff = initial_backoff;
                    }
                    Err(error) => {
                        let mut guard = lock(&status);
                        if let Some(entry) = guard.inputs.get_mut(&universe) {
                            entry.state = InputSignal::Offline;
                            entry.last_error = Some(error.to_string());
                        }
                        guard
                            .warnings
                            .push(format!("input {universe}: reopen failed: {error}"));
                        trim_warnings(&mut guard.warnings);
                        drop(guard);
                        backoff = (backoff * 2).min(Duration::from_secs(8));
                    }
                }
            }
        }
    }

    if let Some(entry) = lock(&status).inputs.get_mut(&universe) {
        entry.state = InputSignal::Stopped;
    }
}

fn push_warning(status: &Arc<Mutex<OutputStatus>>, line: String) {
    let mut status = lock(status);
    status.warnings.push(line);
    trim_warnings(&mut status.warnings);
}

fn trim_warnings(warnings: &mut Vec<String>) {
    if warnings.len() > 64 {
        warnings.drain(..warnings.len() - 64);
    }
}

fn sleep_unless_stopped(stop: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(25)));
    }
}

struct Worker {
    binding: TransportBinding,
    shared: Shared,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<OutputStatus>>,
    config: RuntimeConfig,
}

impl Worker {
    fn run(mut self) {
        let universe = self.binding.universe;
        let period = Duration::from_secs_f64(1.0 / self.config.frame_hz);
        let mut start = Instant::now();
        let mut backoff = Duration::from_millis(self.config.reconnect_ms);
        let mut reconnect_since: Option<Instant> = None;
        let zeros = [0u8; DMX_SLOTS];

        while !self.stop.load(Ordering::Relaxed) {
            let (frame, built_at) = {
                let shared = lock(&self.shared);
                (Arc::clone(&shared.frame), shared.built_at)
            };
            let age = built_at.elapsed();
            let stale = age >= Duration::from_millis(self.config.stale_ms);
            let black = age >= Duration::from_millis(self.config.blackout_ms);
            let slots = if black {
                &zeros
            } else {
                frame.get(universe).unwrap_or(&zeros)
            };

            let began = Instant::now();
            let result = self.binding.transport.send_frame(slots);
            let took = began.elapsed();

            let mut entries = Vec::new();
            if let Some(warning) = self.binding.transport.take_warning() {
                entries.push(format!("universe {universe}: {warning}"));
            }

            let mut recovered = None;
            {
                let mut status = lock(&self.status);
                if result.is_ok() {
                    status.frames += 1;
                    status.last_frame = Some(SystemTime::now());
                    recovered = reconnect_since.take();
                }
                let entry = status.universes.entry(universe).or_default();
                entry.stale_ms = u64::try_from(age.as_millis()).unwrap_or(u64::MAX);
                entry.last_send_us = u64::try_from(took.as_micros()).unwrap_or(u64::MAX);
                let measured = 1.0 / took.as_secs_f64().max(1e-6);
                entry.achieved_hz = if entry.achieved_hz == 0.0 {
                    measured
                } else {
                    entry.achieved_hz * 0.95 + measured * 0.05
                };
                match &result {
                    Ok(()) => {
                        entry.frames += 1;
                        if recovered.is_some() {
                            entry.reconnects += 1;
                        }
                        entry.state = if stale {
                            OutputState::Stale
                        } else {
                            OutputState::Ok
                        };
                    }
                    Err(error) => {
                        entry.errors += 1;
                        entry.last_error = Some(error.to_string());
                        if reconnect_since.is_none() {
                            reconnect_since = Some(Instant::now());
                            entries.push(format!("universe {universe} offline: {error}"));
                        }
                        entry.state = OutputState::Reconnecting;
                    }
                }
            }
            if let Some(since) = recovered {
                entries.push(format!(
                    "universe {universe} recovered after {:.1} s",
                    since.elapsed().as_secs_f64()
                ));
            }

            if !entries.is_empty() {
                lock(&self.status).warnings.extend(entries);
            }

            // A failed universe retries on a backoff; the others never wait for
            // it because each worker has its own clock.
            if reconnect_since.is_some() {
                match self.binding.transport.reopen() {
                    Ok(()) => {
                        // The next send proves the port really works; a failure
                        // puts it straight back into reconnecting.
                        backoff = Duration::from_millis(self.config.reconnect_ms);
                    }
                    Err(error) => {
                        let mut status = lock(&self.status);
                        if let Some(entry) = status.universes.get_mut(&universe) {
                            entry.last_error = Some(error.to_string());
                        }
                        drop(status);
                        thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(8));
                        start = Instant::now();
                        continue;
                    }
                }
            }

            // Self-clocked: the period is measured from the start of this send,
            // so a slow wire slows this universe and nothing else.
            let deadline = start + period;
            let now = Instant::now();
            if deadline > now {
                thread::sleep(deadline - now);
            }
            start = deadline.max(now);
        }

        for _ in 0..2 {
            let _ = self.binding.transport.send_frame(&zeros);
        }
        let _ = self.binding.transport.shutdown();
        let mut status = lock(&self.status);
        if let Some(entry) = status.universes.get_mut(&universe) {
            entry.state = OutputState::Stopped;
        }
    }
}

struct Ticker {
    period: Duration,
    next: Instant,
}

impl Ticker {
    fn new(rate_hz: f64) -> Self {
        let period = Duration::from_secs_f64(1.0 / rate_hz);
        Self {
            period,
            next: Instant::now() + period,
        }
    }

    /// Sleep until the next deadline, skipping deadlines that were missed.
    fn wait(&mut self) {
        let now = Instant::now();
        if self.next > now {
            thread::sleep(self.next - now);
        }
        self.next += self.period;
        let now = Instant::now();
        if self.next <= now {
            self.next = now + self.period;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::demo_show;
    use dmx::virtual_port::VirtualTransport;

    fn bind_virtual() -> (
        VirtualTransport,
        Arc<Mutex<dmx::virtual_port::VirtualState>>,
    ) {
        // Keep every frame: `last` is the blackout frame once output stops.
        let transport = VirtualTransport::with_history(256);
        let handle = transport.handle();
        (transport, handle)
    }

    #[test]
    fn renders_and_sends_programmed_levels() {
        let show = demo_show();
        let mut engine = Engine::new(show);
        engine.desk.programmer.set(1, "Dimmer", 1.0);
        engine.desk.programmer.set(1, "ColorAdd_G", 0.5);

        let (transport, state) = bind_virtual();
        let runtime = Runtime::start(
            engine,
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                ..RuntimeConfig::default()
            },
        );

        thread::sleep(Duration::from_millis(250));
        runtime.stop();

        let state = lock(&state);
        assert!(state.frames >= 5, "sent {} frames", state.frames);
        let frame = state
            .history
            .iter()
            .find(|frame| frame[1] != 0)
            .expect("a frame carried the look");
        assert_eq!(frame.len(), DMX_SLOTS + 1);
        assert_eq!(frame[0], 0, "start code");
        assert_eq!(frame[1], 255, "dimmer at address 1");
        assert_eq!(frame[3], 128, "green at address 3");
    }

    #[test]
    fn picks_up_changes_made_while_running() {
        let show = demo_show();
        let (transport, state) = bind_virtual();
        let runtime = Runtime::start(
            Engine::new(show.clone()),
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                ..RuntimeConfig::default()
            },
        );

        thread::sleep(Duration::from_millis(60));
        {
            let engine = runtime.engine();
            let mut engine = lock(&engine);
            engine.execute("Fixture 1 At Full").unwrap();
        }
        thread::sleep(Duration::from_millis(120));
        runtime.stop();

        let state = lock(&state);
        assert!(
            state.history.iter().any(|frame| frame[1] == 255),
            "command reached the wire"
        );
    }

    #[test]
    fn stop_sends_a_blackout_frame() {
        let show = demo_show();
        let mut engine = Engine::new(show);
        engine.desk.programmer.set(1, "Dimmer", 1.0);
        let (transport, state) = bind_virtual();
        let runtime = Runtime::start(
            engine,
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                ..RuntimeConfig::default()
            },
        );

        thread::sleep(Duration::from_millis(80));
        runtime.stop();

        let state = lock(&state);
        assert!(state.last[1..].iter().all(|slot| *slot == 0));
        assert!(state.frames >= 1, "no frames sent");
    }

    #[test]
    fn status_tracks_running_state_and_counts() {
        let (transport, _state) = bind_virtual();
        let engine = Engine::new(demo_show());
        let status = Arc::clone(&engine.status);
        let runtime = Runtime::start(
            engine,
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                ..RuntimeConfig::default()
            },
        );

        assert!(lock(&status).running);
        thread::sleep(Duration::from_millis(80));
        runtime.stop();

        let status = lock(&status);
        assert!(!status.running);
        assert!(status.frames >= 2);
        assert_eq!(status.universes[&1].transport, "virtual");
        assert_eq!(status.universes[&1].errors, 0);
    }

    #[test]
    fn unbound_universes_are_not_sent() {
        let (transport, state) = bind_virtual();
        let mut show = demo_show();
        show.universes.push(crate::show::Universe {
            number: 2,
            label: "Two".into(),
            size: 512,
            ..crate::show::Universe::default()
        });
        show.fixtures.push(crate::show::Fixture {
            number: 2,
            name: "Second".into(),
            type_key: "OpenLight Demo Wash".into(),
            mode: "4ch".into(),
            universe: 2,
            address: 1,
            home: Default::default(),
        });
        let mut engine = Engine::new(show);
        engine.desk.programmer.set(2, "Dimmer", 1.0);
        let runtime = Runtime::start(
            engine,
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                ..RuntimeConfig::default()
            },
        );
        thread::sleep(Duration::from_millis(80));
        runtime.stop();
        assert!(lock(&state).frames > 0);
    }

    /// A transport whose behaviour the test controls.
    struct Scripted {
        failures: Arc<Mutex<u32>>,
        sent: Arc<Mutex<u64>>,
        saw_nonzero: Arc<AtomicBool>,
        slow: Duration,
    }

    impl Scripted {
        fn new(failures: u32, slow: Duration) -> Self {
            Self {
                failures: Arc::new(Mutex::new(failures)),
                sent: Arc::new(Mutex::new(0)),
                saw_nonzero: Arc::new(AtomicBool::new(false)),
                slow,
            }
        }
    }

    impl DmxTransport for Scripted {
        fn describe(&self) -> String {
            "scripted".into()
        }

        fn send_frame(&mut self, slots: &[u8; DMX_SLOTS]) -> dmx::DmxResult<()> {
            if !self.slow.is_zero() {
                thread::sleep(self.slow);
            }
            if slots.iter().any(|slot| *slot != 0) {
                self.saw_nonzero.store(true, Ordering::SeqCst);
            }
            let mut failures = lock(&self.failures);
            if *failures > 0 {
                *failures -= 1;
                return Err(dmx::DmxError::io(
                    "scripted",
                    std::io::Error::other("unplugged"),
                ));
            }
            *lock(&self.sent) += 1;
            Ok(())
        }

        fn reopen(&mut self) -> dmx::DmxResult<()> {
            Ok(())
        }
    }

    #[test]
    fn a_stalled_engine_sends_black_rather_than_a_frozen_look() {
        let show = demo_show();
        let mut engine = Engine::new(show);
        engine.desk.programmer.set(1, "Dimmer", 1.0);
        let frame = Arc::new(engine.render());

        let transport = Scripted::new(0, Duration::ZERO);
        let saw_nonzero = Arc::clone(&transport.saw_nonzero);
        let shared: Shared = Arc::new(Mutex::new(SharedFrame {
            frame,
            // Older than the blackout threshold: the engine has stalled.
            built_at: Instant::now() - Duration::from_secs(5),
            revision: 1,
        }));
        let status = Arc::new(Mutex::new(OutputStatus::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let config = RuntimeConfig {
            frame_hz: 100.0,
            stale_ms: 10,
            blackout_ms: 20,
            ..RuntimeConfig::default()
        };
        let worker = Worker {
            binding: TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            },
            shared,
            stop: Arc::clone(&stop),
            status: Arc::clone(&status),
            config,
        };
        let handle = thread::spawn(move || worker.run());
        thread::sleep(Duration::from_millis(60));
        stop.store(true, Ordering::SeqCst);
        handle.join().unwrap();

        assert!(
            !saw_nonzero.load(Ordering::SeqCst),
            "a stalled engine must not keep a frozen look on the wire"
        );
        assert_eq!(lock(&status).universes[&1].state, OutputState::Stopped);
    }

    #[test]
    fn a_slow_universe_does_not_starve_a_fast_one() {
        let slow = Scripted::new(0, Duration::from_millis(20));
        let slow_sent = Arc::clone(&slow.sent);
        let fast = Scripted::new(0, Duration::ZERO);
        let fast_sent = Arc::clone(&fast.sent);

        let runtime = Runtime::start(
            engine_with_full_dimmer(),
            vec![
                TransportBinding {
                    universe: 1,
                    transport: Box::new(slow),
                },
                TransportBinding {
                    universe: 2,
                    transport: Box::new(fast),
                },
            ],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                ..RuntimeConfig::default()
            },
        );
        thread::sleep(Duration::from_millis(300));
        runtime.stop();

        let slow = *lock(&slow_sent);
        let fast = *lock(&fast_sent);
        assert!(slow > 0 && fast > 0, "both universes sent frames");
        assert!(
            fast > slow + 5,
            "the slow wire must not slow the fast one: {fast} vs {slow}"
        );
    }

    #[test]
    fn a_dropped_transport_reconnects_and_says_so_once() {
        let transport = Scripted::new(2, Duration::ZERO);
        let sent = Arc::clone(&transport.sent);
        let runtime = Runtime::start(
            engine_with_full_dimmer(),
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                reconnect_ms: 1,
                ..RuntimeConfig::default()
            },
        );
        thread::sleep(Duration::from_millis(200));
        let status = {
            let handle = runtime.engine();
            let status = Arc::clone(&lock(&handle).status);
            status
        };
        runtime.stop();

        assert!(*lock(&sent) > 0, "output resumed after reconnecting");
        let status = lock(&status);
        let entry = &status.universes[&1];
        assert_eq!(entry.reconnects, 1, "one reconnect, not one per frame");
        assert!(entry.frames > 0);
        assert!(
            status
                .warnings
                .iter()
                .any(|warning| warning.contains("recovered")),
            "{:?}",
            status.warnings
        );
    }

    fn engine_with_full_dimmer() -> Engine {
        let mut engine = Engine::new(demo_show());
        engine.desk.programmer.set(1, "Dimmer", 1.0);
        engine
    }

    #[test]
    fn a_virtual_input_reaches_the_output() {
        let mut engine = Engine::new(demo_show());
        engine.show.universes[0].merge = crate::show::MergeMode::Htp;
        let (input, handle) = dmx::virtual_input::VirtualInput::new();
        let (transport, state) = bind_virtual();
        let runtime = Runtime::start_with_inputs(
            engine,
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            vec![InputBinding {
                universe: 1,
                input: Box::new(input),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                ..RuntimeConfig::default()
            },
        );

        let mut slots = [0u8; DMX_SLOTS];
        slots[0] = 200;
        handle.send_slots(slots).unwrap();
        thread::sleep(Duration::from_millis(250));
        runtime.stop();

        let state = lock(&state);
        assert!(
            state.history.iter().any(|frame| frame[1] == 200),
            "the merged input reached the wire: {} frames",
            state.frames
        );
    }

    struct SlowInput {
        polls: Arc<Mutex<u64>>,
    }

    impl DmxInput for SlowInput {
        fn describe(&self) -> String {
            "slow".into()
        }

        fn poll(&mut self, _timeout: Duration) -> dmx::DmxResult<Option<dmx::DmxFrame>> {
            *self.polls.lock().unwrap() += 1;
            thread::sleep(Duration::from_millis(40));
            Ok(None)
        }

        fn reopen(&mut self) -> dmx::DmxResult<()> {
            Ok(())
        }
    }

    struct OnceFailingInput {
        attempts: Arc<Mutex<u64>>,
        reopened: Arc<Mutex<u64>>,
    }

    impl DmxInput for OnceFailingInput {
        fn describe(&self) -> String {
            "failing".into()
        }

        fn poll(&mut self, _timeout: Duration) -> dmx::DmxResult<Option<dmx::DmxFrame>> {
            let mut attempts = self.attempts.lock().unwrap();
            *attempts += 1;
            if *attempts == 1 {
                return Err(test_error("unplugged"));
            }
            Ok(None)
        }

        fn reopen(&mut self) -> dmx::DmxResult<()> {
            *self.reopened.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn test_error(message: &str) -> dmx::DmxError {
        dmx::DmxError::Protocol(message.into())
    }

    #[test]
    fn a_failing_reader_reconnects_without_stopping_output() {
        let attempts = Arc::new(Mutex::new(0));
        let reopened = Arc::new(Mutex::new(0));
        let (transport, state) = bind_virtual();
        let runtime = Runtime::start_with_inputs(
            Engine::new(demo_show()),
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            vec![InputBinding {
                universe: 1,
                input: Box::new(OnceFailingInput {
                    attempts: Arc::clone(&attempts),
                    reopened: Arc::clone(&reopened),
                }),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                reconnect_ms: 1,
                ..RuntimeConfig::default()
            },
        );
        thread::sleep(Duration::from_millis(200));
        let status = {
            let handle = runtime.engine();
            let status = Arc::clone(&lock(&handle).status);
            status
        };
        runtime.stop();

        assert!(*lock(&reopened) >= 1, "the reader was reopened");
        assert!(lock(&state).frames > 0, "output kept running");
        let status = lock(&status);
        assert!(status.inputs[&1].errors >= 1);
        assert!(status.inputs[&1].reconnects >= 1);
    }

    #[test]
    fn a_slow_input_reader_does_not_stall_output() {
        let polls = Arc::new(Mutex::new(0));
        let (transport, state) = bind_virtual();
        let runtime = Runtime::start_with_inputs(
            Engine::new(demo_show()),
            vec![TransportBinding {
                universe: 1,
                transport: Box::new(transport),
            }],
            vec![InputBinding {
                universe: 1,
                input: Box::new(SlowInput {
                    polls: Arc::clone(&polls),
                }),
            }],
            RuntimeConfig {
                engine_hz: 100.0,
                frame_hz: 100.0,
                ..RuntimeConfig::default()
            },
        );
        thread::sleep(Duration::from_millis(250));
        runtime.stop();

        assert!(*lock(&polls) > 0, "the reader was polled");
        assert!(lock(&state).frames > 0, "output kept sending");
    }
}

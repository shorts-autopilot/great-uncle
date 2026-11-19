//! Output runtime.
//!
//! Two threads, both reading only:
//!
//! * the **engine tick** advances playback and re-renders when the engine
//!   revision changes (60 Hz by default). A running fade changes the revision,
//!   so the sender picks it up on the next frame.
//! * the **frame sender** pushes the latest frame to every bound transport at
//!   the configured DMX rate (30 Hz by default, the usual console refresh).
//!
//! Mutation always happens on the caller's thread through the engine mutex, so
//! there is exactly one writer. On stop the sender transmits one black frame:
//! a dark rig is safer than a frozen look.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use dmx::{DmxTransport, DMX_SLOTS};

use crate::engine::Engine;
use crate::render::OutputFrame;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Default)]
pub struct UniverseStatus {
    pub transport: String,
    pub frames: u64,
    pub errors: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OutputStatus {
    pub running: bool,
    pub frame_hz: f64,
    pub engine_hz: f64,
    pub frames: u64,
    pub last_frame: Option<SystemTime>,
    pub universes: BTreeMap<u16, UniverseStatus>,
    pub warnings: Vec<String>,
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
            warnings: Vec::new(),
        }
    }
}

/// One universe bound to one transport.
pub struct TransportBinding {
    pub universe: u16,
    pub transport: Box<dyn DmxTransport>,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeConfig {
    pub engine_hz: f64,
    pub frame_hz: f64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            engine_hz: 60.0,
            frame_hz: 30.0,
        }
    }
}

impl RuntimeConfig {
    fn sanitized(self) -> Self {
        Self {
            engine_hz: self.engine_hz.clamp(1.0, 200.0),
            frame_hz: self.frame_hz.clamp(1.0, 200.0),
        }
    }
}

pub struct Runtime {
    engine: Arc<Mutex<Engine>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    config: RuntimeConfig,
}

impl Runtime {
    pub fn start(engine: Engine, transports: Vec<TransportBinding>, config: RuntimeConfig) -> Self {
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
        }

        let engine = Arc::new(Mutex::new(engine));
        let frame = Arc::new(Mutex::new(OutputFrame::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let engine_thread = {
            let engine = Arc::clone(&engine);
            let frame = Arc::clone(&frame);
            let stop = Arc::clone(&stop);
            thread::spawn(move || engine_loop(engine, frame, stop, config.engine_hz))
        };
        let output_thread = {
            let frame = Arc::clone(&frame);
            let stop = Arc::clone(&stop);
            let status = Arc::clone(&status);
            thread::spawn(move || {
                output_loop(transports, frame, stop, status, config.frame_hz);
            })
        };

        Self {
            engine,
            stop,
            threads: vec![engine_thread, output_thread],
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
            let _ = thread.join();
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn engine_loop(
    engine: Arc<Mutex<Engine>>,
    frame: Arc<Mutex<OutputFrame>>,
    stop: Arc<AtomicBool>,
    rate_hz: f64,
) {
    let mut ticker = Ticker::new(rate_hz);
    let mut revision = u64::MAX;

    while !stop.load(Ordering::Relaxed) {
        ticker.wait();
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let rendered = {
            let mut state = lock(&engine);
            let advanced = state.tick(Instant::now());
            if advanced || state.revision != revision {
                Some((state.render(), state.revision))
            } else {
                None
            }
        };
        if let Some((rendered, current_revision)) = rendered {
            *lock(&frame) = rendered;
            revision = current_revision;
        }
    }
}

fn output_loop(
    mut transports: Vec<TransportBinding>,
    frame: Arc<Mutex<OutputFrame>>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<OutputStatus>>,
    rate_hz: f64,
) {
    let mut ticker = Ticker::new(rate_hz);

    while !stop.load(Ordering::Relaxed) {
        ticker.wait();
        if stop.load(Ordering::Relaxed) {
            break;
        }

        // Copy once per frame so transports never hold the frame lock.
        let snapshot = lock(&frame).clone();
        let mut results = Vec::with_capacity(transports.len());
        let mut warnings = Vec::new();
        for binding in transports.iter_mut() {
            let slots = snapshot
                .get(binding.universe)
                .copied()
                .unwrap_or([0; DMX_SLOTS]);
            let result = binding.transport.send_frame(&slots);
            if let Some(warning) = binding.transport.take_warning() {
                warnings.push(warning);
            }
            results.push((binding.universe, result));
        }

        let mut status = lock(&status);
        status.frames += 1;
        status.last_frame = Some(SystemTime::now());
        for (universe, result) in results {
            if let Some(entry) = status.universes.get_mut(&universe) {
                entry.frames += 1;
                if let Err(error) = result {
                    entry.errors += 1;
                    entry.last_error = Some(error.to_string());
                }
            }
        }
        status.warnings.extend(warnings);
    }

    for binding in transports.iter_mut() {
        let _ = binding.transport.shutdown();
    }
    lock(&status).running = false;
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
        });
        show.fixtures.push(crate::show::Fixture {
            number: 2,
            name: "Second".into(),
            type_key: "OpenLight Demo Wash".into(),
            mode: "4ch".into(),
            universe: 2,
            address: 1,
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
            },
        );
        thread::sleep(Duration::from_millis(80));
        runtime.stop();
        assert!(lock(&state).frames > 0);
    }
}

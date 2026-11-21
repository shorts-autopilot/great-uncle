//! The wiring every OpenLight front end shares.
//!
//! A terminal shell, a desktop window and a test all need the same sequence:
//! load the show and desk, recover the journal, import GDTF/MVR files, bind
//! transports and start the runtime. [`App`] owns that sequence once. Front
//! ends turn input into command text and hand it to [`App::execute`]; they draw
//! from [`App::snapshot`] and never hold the engine lock while drawing.
//!
//! There is deliberately no mutation API other than `execute`. A widget that
//! needs a value produces a command (`Executor 1 At 63`), not a field write;
//! `App` exposes the output status for telemetry but not the engine itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dmx::virtual_port::{VirtualState, VirtualTransport};
use dmx::DmxTransport;
use engine::listing::Listings;
use engine::render::OutputFrame;
use engine::{Engine, Runtime, RuntimeConfig, TransportBinding};

pub use engine::listing::{
    PlaybackRow, PoolRow, RowRun, SheetCell, SheetRow, TimecodeRow, TimecodeRowKind,
};
pub use engine::playback::format_cue_number;
pub use engine::{CommandError, CommandOutcome, RecoveryReport};

/// Where the journal goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum JournalChoice {
    /// `<show>.journal` when a show is open, off otherwise.
    #[default]
    Default,
    Off,
    Path(PathBuf),
}

/// Everything a front end can ask for at start.
#[derive(Debug, Clone)]
pub struct AppOptions {
    pub show: Option<PathBuf>,
    pub desk: Option<PathBuf>,
    pub mvr: Option<PathBuf>,
    pub gdtf: Vec<PathBuf>,
    /// `(universe, transport spec)`. Empty means one virtual universe.
    pub outputs: Vec<(u16, String)>,
    /// Snapshot interval in seconds. `None` or zero disables autosave.
    pub autosave: Option<f64>,
    pub journal: JournalChoice,
    pub recover: bool,
    pub engine_hz: f64,
    pub frame_hz: f64,
    /// Start the output threads. `false` keeps a model-only front end (a
    /// `--status` run or a headless test) from opening a transport.
    pub start_runtime: bool,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            show: None,
            desk: None,
            mvr: None,
            gdtf: Vec::new(),
            outputs: Vec::new(),
            autosave: None,
            journal: JournalChoice::Default,
            recover: true,
            engine_hz: 60.0,
            frame_hz: 30.0,
            start_runtime: true,
        }
    }
}

/// One executor handle as the UI draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutorView {
    pub number: u16,
    pub sequence: u16,
    pub name: String,
    pub mode: String,
    /// 0..=1.
    pub fader: f64,
    pub flash: bool,
    pub solo: bool,
    /// `idle`, `paused cue 2`, `complete cue 2` or `cue 2 45%`.
    pub state: String,
}

/// Desk handles and transient state, copied out of the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct DeskView {
    /// The active executor page and its name.
    pub page: u16,
    pub page_name: String,
    pub selection: Vec<u16>,
    pub blind: bool,
    pub highlight: bool,
    pub blackout: bool,
    pub lock: bool,
    pub preview: Option<String>,
    pub tempo: f64,
    /// 0..=1.
    pub grand: f64,
    /// 0..=1.
    pub rate: f64,
    pub timecode: String,
    pub executors: Vec<ExecutorView>,
}

/// A playback that is on stage right now.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackView {
    /// `Sequence 1` or `Executor 1`.
    pub target: String,
    pub cue: String,
    pub progress: f64,
    pub paused: bool,
    pub done: bool,
}

/// All owned data a front end needs for one repaint.
///
/// The UI takes the engine lock, builds this, and releases it. Drawing never
/// holds the lock, so a slow repaint cannot stall a fade and the output thread
/// cannot be blocked by the window.
#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub revision: u64,
    pub show_name: String,
    pub listings: Listings,
    /// The rendered frame, for level meters and a channel grid.
    pub frame: OutputFrame,
    pub desk: DeskView,
    pub playing: Vec<PlaybackView>,
}

/// Startup wiring errors are strings: every layer already reports a complete
/// message, and both front ends print it verbatim.
pub type AppError = String;

/// The engine, its runtime and the files this session owns.
pub struct App {
    engine: Arc<Mutex<Engine>>,
    /// Mirrors `engine.revision` without taking the lock.
    revision: Arc<AtomicU64>,
    runtime: Option<Runtime>,
    virtual_states: BTreeMap<u16, Arc<Mutex<VirtualState>>>,
    show_path: Option<PathBuf>,
    journal_path: Option<PathBuf>,
    recovery: Option<RecoveryReport>,
    messages: Vec<String>,
    imports: Vec<(String, CommandOutcome)>,
}

impl App {
    /// Load a show, recover it, import rig files, bind outputs and (optionally)
    /// start the runtime. Messages are collected rather than printed so the
    /// console, the GUI and tests share one order.
    pub fn load(options: AppOptions) -> Result<App, AppError> {
        if options.desk.is_some() && options.show.is_none() {
            return Err("--desk needs a show file".to_string());
        }
        if let Some(seconds) = options.autosave {
            if !seconds.is_finite() || seconds < 0.0 {
                return Err("--autosave needs a number of seconds".to_string());
            }
        }
        let mut messages = Vec::new();
        let mut engine = match &options.show {
            Some(path) => {
                let (engine, warnings) = Engine::open_with_desk(path, options.desk.as_deref())
                    .map_err(|error| error.to_string())?;
                messages.push(format!(
                    "Loaded `{}` from {}",
                    engine.show().name,
                    path.display()
                ));
                for warning in warnings {
                    messages.push(format!("Warning: {warning}"));
                }
                engine
            }
            None => Engine::empty(),
        };
        engine.set_bound_outputs(
            options
                .outputs
                .iter()
                .map(|(_, spec)| spec.clone())
                .collect(),
        );

        let mut journal_path = None;
        let mut recovery = None;
        if let Some(show_path) = &options.show {
            let path = match &options.journal {
                JournalChoice::Off => None,
                JournalChoice::Path(path) => Some(path.clone()),
                JournalChoice::Default => Some(engine::journal::path_for(show_path, ".journal")),
            };
            journal_path = path.clone();
            let autosave_path = engine::journal::path_for(show_path, ".autosave");
            // Even with the journal switched off, its head decides where a
            // snapshot says replay starts. Reading it does not append.
            let recovery_path = path
                .clone()
                .unwrap_or_else(|| engine::journal::path_for(show_path, ".journal"));
            let tail = engine::journal::read(&recovery_path).map_err(|error| error.to_string())?;
            engine.set_journal_head(tail.entries.last().map(|entry| entry.rev).unwrap_or(0));

            // Recovery runs before the journal opens, so replay is never
            // journaled.
            if options.recover {
                let snapshot = engine::journal::Snapshot::load(&autosave_path)
                    .map_err(|error| error.to_string())?;
                let from = match &snapshot {
                    Some(snapshot) => {
                        let (show, desk, warnings) = snapshot.restore();
                        engine.adopt(show, desk);
                        for warning in warnings {
                            messages.push(format!("Warning: {warning}"));
                        }
                        match snapshot.rev {
                            Some(rev) => messages.push(format!(
                                "Recovering from autosave {} (revision {rev})",
                                snapshot.at
                            )),
                            None => messages.push(format!(
                                "Recovering from autosave {} (taken with the journal off)",
                                snapshot.at
                            )),
                        }
                        snapshot.replay_from()
                    }
                    None => engine::journal::ReplayFrom::File,
                };
                let report = engine.recover(&tail, show_path, from);
                messages.extend(report.summary());
                recovery = Some(report);
            }

            if let Some(seconds) = options.autosave {
                let interval = Duration::from_secs_f64(seconds);
                if interval.is_zero() {
                    messages.push("Autosave off".to_string());
                } else {
                    messages.push(format!(
                        "Auto-saving to {} every {seconds} s",
                        autosave_path.display()
                    ));
                    engine.enable_autosave(autosave_path, interval);
                }
            }

            if let Some(path) = journal_path.clone() {
                match engine::journal::Journal::open(path) {
                    Ok(journal) => {
                        messages.push(format!("Journal at {}", journal.path().display()));
                        engine.set_journal(Some(journal));
                    }
                    Err(error) => {
                        messages.push(format!("Warning: journal disabled: {error}"));
                    }
                }
            }
        }

        let mut imports = Vec::new();
        for path in &options.gdtf {
            let line = format!("Import Gdtf {}", quote(&path.to_string_lossy()));
            let outcome = engine.execute(&line).map_err(|error| error.to_string())?;
            imports.push((line, outcome));
        }
        if let Some(path) = &options.mvr {
            let line = format!("Import Mvr {}", quote(&path.to_string_lossy()));
            let outcome = engine.execute(&line).map_err(|error| error.to_string())?;
            imports.push((line, outcome));
        }

        let revision = engine.revision_signal();
        let mut virtual_states = BTreeMap::new();
        let (engine, runtime) = if options.start_runtime {
            let mut outputs = options.outputs.clone();
            if outputs.is_empty() {
                outputs.push((1, "virtual".to_string()));
            }
            let mut bindings = Vec::new();
            for (universe, spec) in &outputs {
                let transport: Box<dyn DmxTransport> = if spec == "virtual" {
                    let virtual_transport = VirtualTransport::new();
                    virtual_states.insert(*universe, virtual_transport.handle());
                    Box::new(virtual_transport)
                } else {
                    dmx::open(spec).map_err(|error| error.to_string())?
                };
                messages.push(format!("Universe {universe} -> {}", transport.describe()));
                bindings.push(TransportBinding {
                    universe: *universe,
                    transport,
                });
            }
            let runtime = Runtime::start(
                engine,
                bindings,
                RuntimeConfig {
                    engine_hz: options.engine_hz,
                    frame_hz: options.frame_hz,
                    ..RuntimeConfig::default()
                },
            );
            (runtime.engine(), Some(runtime))
        } else {
            (Arc::new(Mutex::new(engine)), None)
        };

        Ok(App {
            engine,
            revision,
            runtime,
            virtual_states,
            show_path: options.show,
            journal_path,
            recovery,
            messages,
            imports,
        })
    }

    /// Run one command line exactly as the terminal does.
    pub fn execute(&self, line: &str) -> Result<CommandOutcome, CommandError> {
        lock(&self.engine).execute(line)
    }

    /// Build every listing and a rendered frame under one lock.
    pub fn snapshot(&self) -> Snapshot {
        let engine = lock(&self.engine);
        Snapshot {
            revision: engine.revision,
            show_name: engine.show().name.clone(),
            listings: engine.listings(),
            frame: engine.render(),
            desk: desk_view(&engine),
            playing: playing_views(&engine),
        }
    }

    /// The revision poll a UI uses to skip unchanged snapshots. It is
    /// lock-free, so an idle window does not touch the engine every frame.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// The output threads' counters, for `--frames` and telemetry. This is the
    /// only engine-owned value a front end can reach without `execute`.
    pub fn status(&self) -> Arc<Mutex<engine::OutputStatus>> {
        Arc::clone(&lock(&self.engine).status)
    }

    pub fn runtime(&self) -> Option<&Runtime> {
        self.runtime.as_ref()
    }

    /// The state the virtual transport for `universe` sent, for `--trace`.
    pub fn virtual_state(&self, universe: u16) -> Option<Arc<Mutex<VirtualState>>> {
        self.virtual_states.get(&universe).cloned()
    }

    pub fn show_path(&self) -> Option<&Path> {
        self.show_path.as_deref()
    }

    pub fn journal_path(&self) -> Option<&Path> {
        self.journal_path.as_deref()
    }

    pub fn recovery(&self) -> Option<&RecoveryReport> {
        self.recovery.as_ref()
    }

    /// Startup lines in the order the terminal prints them.
    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    /// `(command line, outcome)` for each `--gdtf` and `--mvr` import.
    pub fn imports(&self) -> &[(String, CommandOutcome)] {
        &self.imports
    }

    /// Stop the output threads, if they were started.
    pub fn stop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.stop();
        }
    }
}

fn desk_view(engine: &Engine) -> DeskView {
    let desk = &engine.desk;
    let page = desk.page;
    let page_name = engine
        .show()
        .page(page)
        .map(|page| page.name.clone())
        .unwrap_or_default();
    let executors = engine
        .show()
        .executors_on(page)
        .map(|executor| {
            let state = desk.executor_state(page, executor.number);
            let run = engine
                .playback
                .run(engine::Target::executor(page, executor.number));
            ExecutorView {
                number: executor.number,
                sequence: executor.sequence,
                name: executor.name.clone(),
                mode: executor.mode.label().to_string(),
                fader: state.fader,
                flash: state.flash,
                solo: state.solo,
                state: run_state(run, engine.clock()),
            }
        })
        .collect();
    DeskView {
        page,
        page_name,
        selection: desk.selection.clone(),
        blind: desk.blind,
        highlight: desk.highlight,
        blackout: desk.is_blacked_out(engine.clock()),
        lock: desk.lock.is_some(),
        preview: desk.preview.as_ref().map(|preview| preview.label()),
        tempo: desk.tempo,
        grand: desk.masters.grand,
        rate: desk.masters.rate,
        timecode: engine.timecode_status(),
        executors,
    }
}

fn run_state(run: Option<&engine::playback::Run>, now: Instant) -> String {
    let Some(run) = run else {
        return "idle".to_string();
    };
    let cue = engine::playback::format_cue_number(run.cue_number);
    if run.is_paused() {
        format!("paused cue {cue}")
    } else if run.done {
        format!("complete cue {cue}")
    } else {
        format!("cue {cue} {:.0}%", run.progress(now) * 100.0)
    }
}

fn playing_views(engine: &Engine) -> Vec<PlaybackView> {
    engine
        .playback
        .runs()
        .map(|(target, run)| PlaybackView {
            target: target.label(),
            cue: engine::playback::format_cue_number(run.cue_number),
            progress: run.progress(engine.clock()),
            paused: run.is_paused(),
            done: run.done,
        })
        .collect()
}

fn quote(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::load(AppOptions {
            start_runtime: false,
            ..AppOptions::default()
        })
        .unwrap()
    }

    #[test]
    fn a_desk_without_a_show_is_refused() {
        let result = App::load(AppOptions {
            desk: Some(PathBuf::from("look.oldesk")),
            start_runtime: false,
            ..AppOptions::default()
        });
        match result {
            Ok(_) => panic!("a desk without a show must not load"),
            Err(error) => assert_eq!(error, "--desk needs a show file"),
        }
    }

    #[test]
    fn execute_and_snapshot_leave_the_engine_alone() {
        let app = app();
        let before_revision = app.revision();
        app.execute("Master Grand At 50").unwrap();
        assert!(
            app.revision() > before_revision,
            "the lock-free poll must see the edit"
        );
        let before = {
            let engine = lock(&app.engine);
            (engine.revision, engine.show().clone(), engine.desk.clone())
        };
        let snapshot = app.snapshot();
        assert_eq!(snapshot.revision, before.0);
        assert_eq!(snapshot.revision, app.revision());
        assert_eq!(snapshot.show_name, before.1.name);
        assert_eq!(snapshot.desk.grand, 0.5);
        let after = {
            let engine = lock(&app.engine);
            (engine.revision, engine.show().clone(), engine.desk.clone())
        };
        assert_eq!(before, after, "a snapshot is read-only");
    }

    #[test]
    fn two_snapshots_at_one_revision_are_equal() {
        let app = app();
        app.execute("Master Grand At 25").unwrap();
        let first = app.snapshot();
        let second = app.snapshot();
        assert_eq!(first.revision, second.revision);
        assert_eq!(first.show_name, second.show_name);
        assert_eq!(first.listings, second.listings);
        assert_eq!(first.frame, second.frame);
        assert_eq!(first.desk, second.desk);
        assert_eq!(first.playing, second.playing);
    }

    #[test]
    fn a_running_fade_changes_the_frame_not_the_desk() {
        let gdtf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gdtf/openlight-demo-wash.gdtf");
        let mut app = App::load(AppOptions {
            gdtf: vec![gdtf],
            start_runtime: true,
            journal: JournalChoice::Off,
            frame_hz: 60.0,
            ..AppOptions::default()
        })
        .unwrap();
        app.execute("Patch Fixture 1 At 1.001 Type \"OpenLight Demo Wash\" Mode \"4ch\"")
            .unwrap();
        app.execute("Fixture 1 At Full").unwrap();
        app.execute("Store Cue 1 Fade 2").unwrap();
        app.execute("Clear Values").unwrap();
        app.execute("Go Sequence 1").unwrap();
        let first = app.snapshot();
        std::thread::sleep(Duration::from_millis(120));
        let second = app.snapshot();
        assert_ne!(
            first.frame, second.frame,
            "the running fade should move the dimmer"
        );
        assert_eq!(first.desk, second.desk, "desk handles did not move");
        assert_eq!(first.show_name, second.show_name);
        assert!(
            second.frame.slot(1, 1).unwrap_or(0) > 0,
            "the fade should be above zero after 120 ms: {:?}",
            second.frame.universes().collect::<Vec<_>>()
        );
        app.stop();
    }

    #[test]
    fn the_demo_script_produces_the_same_outcomes_through_app_and_engine() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/phase1-demo.txt");
        let gdtf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gdtf/openlight-demo-wash.gdtf");
        let text = std::fs::read_to_string(&path).unwrap();
        let app = app();
        let mut engine = Engine::empty();
        for line in text.lines() {
            let mut line = line.trim().to_string();
            if line.is_empty() || line.starts_with('#') || line.starts_with("Save ") {
                continue;
            }
            line = line.replace(
                "examples/gdtf/openlight-demo-wash.gdtf",
                &gdtf.display().to_string(),
            );
            let line = line.as_str();
            let from_app = app.execute(line).unwrap_or_else(|error| {
                panic!("App failed on `{line}`: {error}");
            });
            let from_engine = engine.execute(line).unwrap_or_else(|error| {
                panic!("Engine failed on `{line}`: {error}");
            });
            assert_eq!(from_app, from_engine, "outcome differs for `{line}`");
        }
        assert_eq!(app.snapshot().show_name, engine.show().name);
    }

    #[test]
    fn startup_messages_name_the_show_the_journal_and_the_universe() {
        let path =
            std::env::temp_dir().join(format!("openlight-app-{}-show.olshow", std::process::id()));
        engine::Show::new().save(&path).unwrap();
        let mut app = App::load(AppOptions {
            show: Some(path.clone()),
            start_runtime: true,
            journal: JournalChoice::Off,
            outputs: vec![(1, "virtual".into())],
            ..AppOptions::default()
        })
        .unwrap();
        let messages = app.messages().join("\n");
        assert!(messages.contains("Loaded `Untitled Show`"), "{messages}");
        assert!(messages.contains("Universe 1 -> virtual"), "{messages}");
        assert!(app.journal_path().is_none());
        assert_eq!(app.show_path(), Some(path.as_path()));
        app.stop();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn imports_are_reported_with_the_line_that_ran() {
        let gdtf = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gdtf/openlight-demo-wash.gdtf");
        let app = App::load(AppOptions {
            gdtf: vec![gdtf],
            start_runtime: false,
            journal: JournalChoice::Off,
            ..AppOptions::default()
        })
        .unwrap();
        let (line, outcome) = &app.imports()[0];
        assert!(line.starts_with("Import Gdtf "), "{line}");
        assert!(
            outcome
                .lines
                .join(" ")
                .contains("Imported 1 new fixture type"),
            "{outcome:?}"
        );
        assert!(app.snapshot().listings.pools.is_empty());
    }

    #[test]
    fn the_desk_view_shows_only_the_active_page() {
        let app = app();
        app.execute("Store Page 2 \"Specials\"").unwrap();
        app.execute("Page 2").unwrap();

        let snapshot = app.snapshot();
        assert_eq!(snapshot.desk.page, 2);
        assert_eq!(snapshot.desk.page_name, "Specials");
        assert!(
            snapshot.desk.executors.is_empty(),
            "an empty page shows no page-one executors"
        );

        app.execute("Page 1").unwrap();
        assert_eq!(app.snapshot().desk.page, 1);
    }
}

//! `openlight` — the command line console.
//!
//! Every command goes through [`engine::Engine::execute`], the same path the
//! GUI and scripting surfaces will use. `Wait <seconds>` is handled here, by
//! the script runner, so a script can give a fade real time to run.
//!
//! `--ui` opens the terminal shell (see [`ui`]); without it the console is a
//! line-oriented REPL.

use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dmx::virtual_port::{VirtualState, VirtualTransport};
use dmx::DmxTransport;
use engine::{Engine, Runtime, RuntimeConfig, Show, TransportBinding};

mod ui;

const USAGE: &str = "\
openlight — DMX lighting console (Phase 4)

Usage: openlight [options]

Options:
  --show <file>        load a show file at start
  --gdtf <file>        import a GDTF fixture type (repeatable)
  --output <spec>      bind a universe to an output, e.g. 1=virtual,
                       1=generic:/dev/ttyUSB0, 1=enttec:/dev/ttyUSB0, 1=udmx
                       (repeatable, defaults to 1=virtual)
  --script <file>      run commands from a file, one per line (# comments);
                       `Wait <seconds>` pauses the script during a fade
  --frames <n>         run for n output frames, then exit
  --fps <n>            frame rate (default 30)
  --engine-hz <n>      engine tick rate (default 60)
  --ui                 open the full-screen terminal shell
  --trace              print the last virtual frame as bytes
  --help               show this text

Examples:
  openlight --ui
  openlight --gdtf examples/gdtf/openlight-demo-wash.gdtf \\
            --output 1=virtual --script examples/phase1-demo.txt
  openlight --output 1=virtual --script examples/phase2-demo.txt \\
            --frames 90 --trace
  openlight --output 1=virtual --script examples/phase3-demo.txt \\
            --frames 120 --trace
  openlight --output 1=virtual --script examples/phase4-demo.txt \\
            --frames 120 --trace
  openlight --output 1=generic:/dev/ttyUSB0 --script examples/phase2-demo.txt --frames 90
";

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1).collect()) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    show: Option<PathBuf>,
    gdtf: Vec<PathBuf>,
    outputs: Vec<(u16, String)>,
    frames: Option<u64>,
    fps: f64,
    engine_hz: f64,
    script: Option<PathBuf>,
    trace: bool,
    ui: bool,
}

impl Options {
    fn parse(arguments: Vec<String>) -> Result<Option<Options>, String> {
        let mut options = Options {
            show: None,
            gdtf: Vec::new(),
            outputs: Vec::new(),
            frames: None,
            fps: 30.0,
            engine_hz: 60.0,
            script: None,
            trace: false,
            ui: false,
        };

        let mut index = 0;
        while index < arguments.len() {
            let argument = &arguments[index];
            let (flag, inline) = match argument.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_string())),
                None => (argument.as_str(), None),
            };
            match flag {
                "--show" => path_option(
                    &mut options.show,
                    take_value(&arguments, &mut index, inline, flag)?,
                ),
                "--gdtf" => options.gdtf.push(PathBuf::from(take_value(
                    &arguments, &mut index, inline, flag,
                )?)),
                "--output" => options.outputs.push(parse_output(&take_value(
                    &arguments, &mut index, inline, flag,
                )?)?),
                "--script" => path_option(
                    &mut options.script,
                    take_value(&arguments, &mut index, inline, flag)?,
                ),
                "--frames" => {
                    options.frames = Some(
                        take_value(&arguments, &mut index, inline, flag)?
                            .parse()
                            .map_err(|_| "--frames needs a whole number".to_string())?,
                    )
                }
                "--fps" => {
                    options.fps = take_value(&arguments, &mut index, inline, flag)?
                        .parse()
                        .map_err(|_| "--fps needs a number".to_string())?
                }
                "--engine-hz" => {
                    options.engine_hz = take_value(&arguments, &mut index, inline, flag)?
                        .parse()
                        .map_err(|_| "--engine-hz needs a number".to_string())?
                }
                "--trace" => options.trace = true,
                "--ui" => options.ui = true,
                "--help" | "-h" => return Ok(None),
                other => return Err(format!("unknown option `{other}`")),
            }
            index += 1;
        }

        if options.outputs.is_empty() {
            options.outputs.push((1, "virtual".to_string()));
        }
        Ok(Some(options))
    }
}

fn path_option(slot: &mut Option<PathBuf>, value: String) {
    *slot = Some(PathBuf::from(value));
}

fn take_value(
    arguments: &[String],
    index: &mut usize,
    inline: Option<String>,
    flag: &str,
) -> Result<String, String> {
    if let Some(value) = inline {
        return Ok(value);
    }
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_output(spec: &str) -> Result<(u16, String), String> {
    match spec.split_once('=') {
        Some((universe, transport)) => {
            let universe: u16 = universe
                .trim()
                .parse()
                .map_err(|_| format!("`{universe}` is not a universe number"))?;
            if universe == 0 {
                return Err("universe 0 is not valid".to_string());
            }
            Ok((universe, transport.trim().to_string()))
        }
        None => Ok((1, spec.trim().to_string())),
    }
}

fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let mut engine = match &options.show {
        Some(path) => {
            let show = Show::load(path)?;
            println!("Loaded `{}` from {}", show.name, path.display());
            Engine::new(show)
        }
        None => Engine::empty(),
    };

    for path in &options.gdtf {
        let outcome = engine.execute(&format!("Import Gdtf {}", quote(&path.to_string_lossy())))?;
        print_outcome(&outcome);
    }

    let mut virtual_states: BTreeMap<u16, Arc<Mutex<VirtualState>>> = BTreeMap::new();
    let mut bindings = Vec::new();
    for (universe, spec) in &options.outputs {
        let transport: Box<dyn DmxTransport> = if spec == "virtual" {
            let virtual_transport = VirtualTransport::new();
            virtual_states.insert(*universe, virtual_transport.handle());
            Box::new(virtual_transport)
        } else {
            dmx::open(spec)?
        };
        println!("Universe {universe} -> {}", transport.describe());
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
            frame_hz: options.fps,
        },
    );

    if let Some(script) = &options.script {
        execute_script(&runtime, script)?;
    }

    if let Some(frames) = options.frames {
        wait_for_frames(&runtime, frames)?;
    } else if options.script.is_none() {
        if options.ui {
            ui::run(&runtime)?;
        } else {
            repl(&runtime)?;
        }
    }

    {
        let handle = runtime.engine();
        let mut engine = lock(&handle);
        let outcome = engine.execute("Status")?;
        print_outcome(&outcome);
    }

    if options.trace {
        for (universe, state) in &virtual_states {
            println!("{}", trace_line(*universe, &lock(state)));
        }
    }

    runtime.stop();
    Ok(())
}

/// Wait until `frames` frames have actually been sent, not just until enough
/// time has passed: the sender may be slower than its nominal rate.
fn wait_for_frames(runtime: &Runtime, frames: u64) -> Result<(), Box<dyn Error>> {
    let handle = runtime.engine();
    let status = {
        let engine = lock(&handle);
        Arc::clone(&engine.status)
    };
    let expected = Duration::from_secs_f64(frames as f64 / runtime.config().frame_hz.max(1.0));
    let deadline = Instant::now() + expected + Duration::from_secs(5);

    while lock(&status).frames < frames {
        if Instant::now() >= deadline {
            return Err(format!(
                "output sent {} of {frames} frames before giving up",
                lock(&status).frames
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn repl(runtime: &Runtime) -> io::Result<()> {
    let handle = runtime.engine();
    println!("Type `Help` for commands, `exit` to quit.");
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        print!("openlight> ");
        io::stdout().flush()?;
        let Some(line) = lines.next() else {
            break;
        };
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
            break;
        }
        if is_wait(trimmed) {
            match parse_wait(trimmed) {
                Some(seconds) => std::thread::sleep(Duration::from_secs_f64(seconds)),
                None => println!("error: `Wait` needs a number of seconds, e.g. `Wait 1.5`"),
            }
            continue;
        }
        match lock(&handle).execute(trimmed) {
            Ok(outcome) => print_outcome(&outcome),
            Err(error) => println!("error: {error}"),
        }
    }
    Ok(())
}

fn execute_script(runtime: &Runtime, path: &Path) -> Result<(), Box<dyn Error>> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let handle = runtime.engine();
    for (line_number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        println!("> {line}");
        if is_wait(line) {
            match parse_wait(line) {
                Some(seconds) => std::thread::sleep(Duration::from_secs_f64(seconds)),
                None => {
                    return Err(format!(
                        "{}:{}: `Wait` needs a number of seconds, e.g. `Wait 1.5`",
                        path.display(),
                        line_number + 1
                    )
                    .into())
                }
            }
            continue;
        }
        match lock(&handle).execute(line) {
            Ok(outcome) => print_outcome(&outcome),
            Err(error) => {
                return Err(format!("{}:{}: {error}", path.display(), line_number + 1).into())
            }
        }
    }
    Ok(())
}

fn is_wait(line: &str) -> bool {
    line.split_whitespace()
        .next()
        .map(|word| word.eq_ignore_ascii_case("wait"))
        .unwrap_or(false)
}

/// `Wait <seconds>` — script-only, bounded so a typo cannot hang a run.
fn parse_wait(line: &str) -> Option<f64> {
    let mut words = line.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("wait") {
        return None;
    }
    let seconds: f64 = words.next()?.parse().ok()?;
    if words.next().is_some() {
        return None;
    }
    if seconds.is_finite() && (0.0..=600.0).contains(&seconds) {
        Some(seconds)
    } else {
        None
    }
}

fn trace_line(universe: u16, state: &VirtualState) -> String {
    let Some(first) = state.last.first() else {
        return format!("Universe {universe}: no frames sent");
    };
    let preview: Vec<String> = state
        .last
        .iter()
        .take(25)
        .map(|byte| format!("{byte:02X}"))
        .collect();
    let non_zero = state.last.iter().skip(1).filter(|byte| **byte != 0).count();
    format!(
        "Universe {universe}: {} frames, last frame {} bytes, start code 0x{first:02X}, \
         {} non-zero slots\n  {}{}",
        state.frames,
        state.last.len(),
        non_zero,
        preview.join(" "),
        if state.last.len() > 25 { " …" } else { "" }
    )
}

fn print_outcome(outcome: &engine::CommandOutcome) {
    for line in &outcome.lines {
        println!("{line}");
    }
}

fn quote(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

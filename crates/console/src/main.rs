//! `openlight` — the command line console.
//!
//! Every command goes through [`engine::Engine::execute`], the same path the
//! GUI and scripting surfaces will use. `Wait <seconds>` is handled here, by
//! the script runner, so a script can give a fade real time to run.
//!
//! `--ui` opens the terminal shell (see [`ui`]); without it the console is a
//! line-oriented REPL.

use std::error::Error;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dmx::virtual_port::VirtualState;
use engine::Engine;
use openlight_app::{App, AppOptions, JournalChoice};

mod completions;
mod session;
mod ui;

use session::SessionLog;

const USAGE: &str = "\
openlight — DMX lighting console (Phase 5)

Usage: openlight [options]

Options:
  --show <file>        load a show file at start
  --desk <file>        load this desk file instead of <show>.oldesk
  --mvr <file>         import an MVR rig before any --script command
  --gdtf <file>        import a GDTF fixture type (repeatable)
  --output <spec>      bind a universe to an output, e.g. 1=virtual,
                       1=generic:/dev/ttyUSB0, 1=enttec:/dev/ttyUSB0, 1=udmx
                       (repeatable, defaults to 1=virtual)
  --input <spec>       bind a universe to an input, e.g. 1=virtual or
                       1=enttec:/dev/ttyUSB0 (repeatable, no input by default)
  --command <line>    run one command before the script (repeatable)
  --script <file>      run commands from a file, or `-` for stdin, one per line
                       (# comments); `Wait <seconds>` pauses during a fade
  --status [--json]    print status once and exit
  --check [file]       load, validate and print `path: problem` lines; no output
  --quiet              suppress startup lines, keep command output
  --log <file>         append every executed line and result to a session log
  --frames <n>         run for n output frames, then exit
  --fps <n>            frame rate (default 30)
  --engine-hz <n>      engine tick rate (default 60)
  --autosave <secs>    snapshot show and desk every <secs> (0 disables)
  --journal <file|off> journal each edit to <file> (default: <show>.journal)
  --no-recover         do not replay the journal at start
  --osc-port <port>    listen for OSC on this UDP port (default 9000)
  --remote-port <port> serve the browser remote on this TCP port (default 8080)
  --remote-bind <ip>   address for OSC and HTTP (default 127.0.0.1)
  --remote-token-file <file>  bearer token file (default ~/.openlight/remote.token)
  --no-remote          do not start OSC or the browser remote
  --ui                 open the full-screen terminal shell
  --trace              print the last virtual frame as bytes
  --version            print the package and show format versions
  --probe              probe serial ports and report DMX framing, then exit
  --completions <sh>   print a completion script for bash, zsh or fish
  --help               show this text

`--command`, a file `--script` and `--frames` compose in that order. `--check`
and `--status` exit without starting output threads; `--check` exits 1 when it
finds a problem.

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
  openlight --show look.olshow --autosave 30 --ui
  openlight --version
  openlight --check look.olshow
  openlight --command \"Fixture 1 At 50\" --command \"Store Cue 1\" --frames 10
  openlight --script - < examples/phase1-demo.txt
  openlight --status --json
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

    if options.version {
        println!("{}", version_line());
        return ExitCode::SUCCESS;
    }
    if let Some(shell) = &options.completions {
        return match completions::script(shell) {
            Ok(script) => {
                print!("{script}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if options.check {
        return check_exit(&options);
    }
    if options.probe {
        return probe_exit(&options);
    }

    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The `--version` line. A release tarball has no git hash, so it prints the
/// package and format only; a checkout adds the commit the binary came from.
fn version_line() -> String {
    let hash = env!("OPENLIGHT_GIT_HASH");
    if hash.is_empty() {
        format!(
            "openlight {} (show format v{})",
            env!("CARGO_PKG_VERSION"),
            engine::SHOW_VERSION
        )
    } else {
        format!(
            "openlight {} ({hash}, show format v{})",
            env!("CARGO_PKG_VERSION"),
            engine::SHOW_VERSION
        )
    }
}

/// `--check`: load the show, validate it and print `path: problem` lines only.
fn check_exit(options: &Options) -> ExitCode {
    let path = options.check_path.as_deref().or(options.show.as_deref());
    let problems = check_show(path);
    for problem in &problems {
        println!("{problem}");
    }
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn check_show(path: Option<&Path>) -> Vec<String> {
    let (show, label) = match path {
        Some(path) => match engine::Show::load_with_report(path) {
            Ok((show, _)) => (show, path.display().to_string()),
            Err(error) => {
                let label = path.display().to_string();
                let error = error.to_string();
                return vec![if error.starts_with(&label) {
                    error
                } else {
                    format!("{label}: {error}")
                }];
            }
        },
        None => (engine::Show::new(), "<empty show>".to_string()),
    };
    show.validate()
        .into_iter()
        .map(|problem| format!("{label}: {problem}"))
        .collect()
}

/// `--probe`: measure every configured output and every serial port this
/// platform knows about, without starting output threads.
fn probe_exit(options: &Options) -> ExitCode {
    let mut specs: Vec<String> = options
        .outputs
        .iter()
        .map(|(_, spec)| spec.clone())
        .collect();
    specs.extend(dmx::probe::candidates());
    let reports = dmx::probe::run(&specs, dmx::probe::ProbeOptions::default());
    if reports.is_empty() {
        println!("No serial ports found; name one with `--output 1=generic:COM3` or `--output 1=generic:/dev/ttyUSB0`.");
    }
    for report in &reports {
        for line in report.lines() {
            println!("{line}");
        }
    }
    ExitCode::SUCCESS
}

struct Options {
    show: Option<PathBuf>,
    desk: Option<PathBuf>,
    mvr: Option<PathBuf>,
    gdtf: Vec<PathBuf>,
    outputs: Vec<(u16, String)>,
    inputs: Vec<(u16, String)>,
    commands: Vec<String>,
    frames: Option<u64>,
    fps: f64,
    engine_hz: f64,
    script: Option<PathBuf>,
    autosave: Option<f64>,
    journal: JournalChoice,
    recover: bool,
    trace: bool,
    ui: bool,
    check: bool,
    check_path: Option<PathBuf>,
    status: bool,
    json: bool,
    quiet: bool,
    log: Option<PathBuf>,
    version: bool,
    probe: bool,
    completions: Option<String>,
    remote: bool,
    osc_port: u16,
    remote_port: u16,
    remote_bind: IpAddr,
    remote_token_file: Option<PathBuf>,
}

impl Options {
    fn parse(arguments: Vec<String>) -> Result<Option<Options>, String> {
        let mut options = Options {
            show: None,
            desk: None,
            mvr: None,
            gdtf: Vec::new(),
            outputs: Vec::new(),
            inputs: Vec::new(),
            commands: Vec::new(),
            frames: None,
            fps: 30.0,
            engine_hz: 60.0,
            script: None,
            autosave: None,
            journal: JournalChoice::Default,
            recover: true,
            trace: false,
            ui: false,
            check: false,
            check_path: None,
            status: false,
            json: false,
            quiet: false,
            log: None,
            version: false,
            probe: false,
            completions: None,
            remote: true,
            osc_port: 9000,
            remote_port: 8080,
            remote_bind: IpAddr::from([127, 0, 0, 1]),
            remote_token_file: None,
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
                "--desk" => path_option(
                    &mut options.desk,
                    take_value(&arguments, &mut index, inline, flag)?,
                ),
                "--command" => options
                    .commands
                    .push(take_value(&arguments, &mut index, inline, flag)?),
                "--status" => options.status = true,
                "--json" => options.json = true,
                "--quiet" => options.quiet = true,
                "--log" => path_option(
                    &mut options.log,
                    take_value(&arguments, &mut index, inline, flag)?,
                ),
                "--version" => options.version = true,
                "--probe" => options.probe = true,
                "--completions" => {
                    options.completions = Some(take_value(&arguments, &mut index, inline, flag)?)
                }
                "--mvr" => path_option(
                    &mut options.mvr,
                    take_value(&arguments, &mut index, inline, flag)?,
                ),
                "--gdtf" => options.gdtf.push(PathBuf::from(take_value(
                    &arguments, &mut index, inline, flag,
                )?)),
                "--output" => options.outputs.push(parse_output(&take_value(
                    &arguments, &mut index, inline, flag,
                )?)?),
                "--input" => options.inputs.push(parse_output(&take_value(
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
                "--autosave" => {
                    let value: f64 = take_value(&arguments, &mut index, inline, flag)?
                        .parse()
                        .map_err(|_| "--autosave needs a number of seconds".to_string())?;
                    if !value.is_finite() || value < 0.0 {
                        return Err("--autosave needs a number of seconds".to_string());
                    }
                    options.autosave = Some(value);
                }
                "--journal" => {
                    let value = take_value(&arguments, &mut index, inline, flag)?;
                    options.journal = if value.eq_ignore_ascii_case("off") {
                        JournalChoice::Off
                    } else {
                        JournalChoice::Path(PathBuf::from(value))
                    };
                }
                "--no-recover" => options.recover = false,
                "--osc-port" => {
                    options.osc_port =
                        parse_port(&take_value(&arguments, &mut index, inline, flag)?)?
                }
                "--remote-port" => {
                    options.remote_port =
                        parse_port(&take_value(&arguments, &mut index, inline, flag)?)?
                }
                "--remote-bind" => {
                    let value = take_value(&arguments, &mut index, inline, flag)?;
                    options.remote_bind = value
                        .parse()
                        .map_err(|_| format!("`{value}` is not an IP address"))?;
                }
                "--remote-token-file" => path_option(
                    &mut options.remote_token_file,
                    take_value(&arguments, &mut index, inline, flag)?,
                ),
                "--no-remote" => options.remote = false,
                "--engine-hz" => {
                    options.engine_hz = take_value(&arguments, &mut index, inline, flag)?
                        .parse()
                        .map_err(|_| "--engine-hz needs a number".to_string())?
                }
                "--trace" => options.trace = true,
                "--ui" => options.ui = true,
                "--check" => {
                    options.check = true;
                    if let Some(value) = inline {
                        if value.trim().is_empty() {
                            return Err("--check needs a show file".to_string());
                        }
                        options.check_path = Some(PathBuf::from(value));
                    } else if let Some(next) = arguments.get(index + 1) {
                        if !next.starts_with('-') {
                            index += 1;
                            options.check_path = Some(PathBuf::from(next));
                        }
                    }
                }
                "--help" | "-h" => return Ok(None),
                other => return Err(format!("unknown option `{other}`")),
            }
            index += 1;
        }

        if options.json && !options.status {
            return Err("--json needs --status".to_string());
        }
        if options.check && options.status {
            return Err("--check and --status cannot be combined".to_string());
        }
        if options.desk.is_some()
            && options.show.is_none()
            && options.check_path.is_none()
            && !options.check
        {
            return Err("--desk needs a show file".to_string());
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

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse()
        .map_err(|_| format!("`{value}` is not a valid port"))
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
    if options.status {
        return status_only(&options);
    }

    let mut log = match options.log.as_deref() {
        Some(path) => Some(SessionLog::open(path)?),
        None => None,
    };

    let mut app = App::load(AppOptions {
        show: options.show.clone(),
        desk: options.desk.clone(),
        mvr: options.mvr.clone(),
        gdtf: options.gdtf.clone(),
        outputs: options.outputs.clone(),
        inputs: options.inputs.clone(),
        autosave: options.autosave,
        journal: options.journal.clone(),
        recover: options.recover,
        engine_hz: options.engine_hz,
        frame_hz: options.fps,
        start_runtime: true,
    })?;

    // Startup lines come from the shared wiring, so the console and the GUI
    // print the same show, journal and transport messages in the same order.
    for line in app.messages() {
        if !options.quiet {
            println!("{line}");
        } else if line.starts_with("Warning:") {
            eprintln!("{line}");
        }
    }
    for (command, outcome) in app.imports() {
        for line in &outcome.lines {
            println!("{line}");
        }
        record_log(&mut log, command, &outcome.lines.join(" | "));
    }

    if let Some(report) = app.recovery() {
        if report.recovered_anything() && !options.quiet && io::stdin().is_terminal() {
            if let Some(show_path) = app.show_path() {
                print!(
                    "The recovered session is not saved. Save it to {}? [Y/n] ",
                    show_path.display()
                );
                io::stdout().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                if !answer.trim().eq_ignore_ascii_case("n") {
                    let save = format!("Save {}", quote(&show_path.to_string_lossy()));
                    let outcome = app.execute(&save)?;
                    print_outcome(&outcome);
                    record_log(&mut log, &save, &outcome.lines.join(" | "));
                }
            }
        }
    }

    let runtime = app.runtime().ok_or("the output runtime did not start")?;

    let remote = if options.remote {
        let config = remote::RemoteConfig {
            osc_port: options.osc_port,
            http_port: options.remote_port,
            bind: options.remote_bind,
            token_path: options
                .remote_token_file
                .clone()
                .unwrap_or_else(remote::token::default_path),
        };
        let handle = remote::Remote::start(runtime.engine(), config)?;
        if !options.quiet {
            println!(
                "OSC listening on {}; remote at http://{} (token in {})",
                handle.osc_addr(),
                handle.http_addr(),
                handle.token_path().display()
            );
            if options.remote_bind.is_unspecified() {
                println!(
                    "Warning: the remote is bound to {}; anyone who can reach this port and has the token can drive the desk",
                    options.remote_bind
                );
            }
        }
        Some(handle)
    } else {
        None
    };

    let has_commands = !options.commands.is_empty();
    for command in &options.commands {
        execute_command_line(&app, command, &mut log)?;
    }
    if let Some(script) = &options.script {
        execute_script(&app, script, options.quiet, &mut log)?;
    }

    if let Some(frames) = options.frames {
        wait_for_frames(&app, frames)?;
    } else if !has_commands && options.script.is_none() {
        if options.ui {
            ui::run(runtime, log.take())?;
        } else {
            repl(&app, &mut log, options.quiet)?;
        }
    }

    let outcome = app.execute("Status")?;
    print_outcome(&outcome);

    if options.trace {
        for (universe, _) in &options.outputs {
            if let Some(state) = app.virtual_state(*universe) {
                println!("{}", trace_line(*universe, &lock(&state)));
            }
        }
    }

    if let Some(remote) = remote {
        remote.stop();
    }
    app.stop();
    Ok(())
}

/// `--status`: load, report and exit without opening a transport or starting a
/// thread. It shares the same journal-path hint the full run would use.
fn status_only(options: &Options) -> Result<(), Box<dyn Error>> {
    let mut engine = match &options.show {
        Some(path) => Engine::open_with_desk(path, options.desk.as_deref())?.0,
        None => Engine::empty(),
    };
    engine.set_bound_outputs(
        options
            .outputs
            .iter()
            .chain(options.inputs.iter())
            .map(|(_, spec)| spec.clone())
            .collect(),
    );
    if let Some(show_path) = &options.show {
        let journal = match &options.journal {
            JournalChoice::Off => None,
            JournalChoice::Path(path) => Some(path.clone()),
            JournalChoice::Default => Some(engine::journal::path_for(show_path, ".journal")),
        };
        engine.set_journal_path_hint(journal);
    }
    if options.json {
        println!("{}", engine.status_json());
    } else {
        let outcome = engine.execute("Status")?;
        print_outcome(&outcome);
    }
    Ok(())
}

/// Wait until `frames` frames have actually been sent, not just until enough
/// time has passed: the sender may be slower than its nominal rate.
fn wait_for_frames(app: &App, frames: u64) -> Result<(), Box<dyn Error>> {
    let status = app.status();
    let frame_hz = app
        .runtime()
        .map(|runtime| runtime.config().frame_hz)
        .unwrap_or(30.0);
    let expected = Duration::from_secs_f64(frames as f64 / frame_hz.max(1.0));
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

fn repl(app: &App, log: &mut Option<SessionLog>, quiet: bool) -> io::Result<()> {
    if !quiet {
        println!("Type `Help` for commands, `exit` to quit.");
    }
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
                Some(seconds) => {
                    std::thread::sleep(Duration::from_secs_f64(seconds));
                    record_log(log, trimmed, &format!("waited {seconds}s"));
                }
                None => println!("error: `Wait` needs a number of seconds, e.g. `Wait 1.5`"),
            }
            continue;
        }
        match app.execute(trimmed) {
            Ok(outcome) => {
                print_outcome(&outcome);
                record_log(log, trimmed, &outcome.lines.join(" | "));
            }
            Err(error) => {
                println!("error: {error}");
                record_log(log, trimmed, &format!("error: {error}"));
            }
        }
    }
    Ok(())
}

fn execute_command_line(
    app: &App,
    line: &str,
    log: &mut Option<SessionLog>,
) -> Result<(), Box<dyn Error>> {
    if is_wait(line) {
        let Some(seconds) = parse_wait(line) else {
            return Err("`Wait` needs a number of seconds, e.g. `Wait 1.5`".into());
        };
        std::thread::sleep(Duration::from_secs_f64(seconds));
        record_log(log, line, &format!("waited {seconds}s"));
        return Ok(());
    }
    let result = match app.execute(line) {
        Ok(outcome) => {
            print_outcome(&outcome);
            record_log(log, line, &outcome.lines.join(" | "));
            Ok(())
        }
        Err(error) => {
            record_log(log, line, &format!("error: {error}"));
            Err(error.into())
        }
    };
    result
}

fn execute_script(
    app: &App,
    path: &Path,
    quiet: bool,
    log: &mut Option<SessionLog>,
) -> Result<(), Box<dyn Error>> {
    let label = if path == Path::new("-") {
        "<stdin>".to_string()
    } else {
        path.display().to_string()
    };
    let text = if path == Path::new("-") {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        text
    } else {
        std::fs::read_to_string(path).map_err(|error| format!("{label}: {error}"))?
    };
    for (line_number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !quiet {
            println!("> {line}");
        }
        if is_wait(line) {
            match parse_wait(line) {
                Some(seconds) => {
                    std::thread::sleep(Duration::from_secs_f64(seconds));
                    record_log(log, line, &format!("waited {seconds}s"));
                }
                None => {
                    return Err(format!(
                        "{label}:{}: `Wait` needs a number of seconds, e.g. `Wait 1.5`",
                        line_number + 1
                    )
                    .into())
                }
            }
            continue;
        }
        match app.execute(line) {
            Ok(outcome) => {
                print_outcome(&outcome);
                record_log(log, line, &outcome.lines.join(" | "));
            }
            Err(error) => {
                record_log(log, line, &format!("error: {error}"));
                return Err(format!("{label}:{}: {error}", line_number + 1).into());
            }
        }
    }
    Ok(())
}

fn record_log(log: &mut Option<SessionLog>, line: &str, result: &str) {
    if let Some(log) = log {
        log.record(line, result);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_flags_parse_and_no_remote_disables_them() {
        let options = Options::parse(vec!["--no-remote".into()]).unwrap().unwrap();
        assert!(!options.remote);

        let options = Options::parse(vec![
            "--osc-port".into(),
            "9001".into(),
            "--remote-port".into(),
            "8081".into(),
            "--remote-bind".into(),
            "0.0.0.0".into(),
            "--remote-token-file".into(),
            "/tmp/openlight.token".into(),
        ])
        .unwrap()
        .unwrap();
        assert!(options.remote);
        assert_eq!(options.osc_port, 9001);
        assert_eq!(options.remote_port, 8081);
        assert!(options.remote_bind.is_unspecified());
        assert_eq!(
            options.remote_token_file.as_deref(),
            Some(Path::new("/tmp/openlight.token"))
        );
    }

    #[test]
    fn mvr_flag_takes_a_path() {
        let options = Options::parse(vec!["--mvr".into(), "rig.mvr".into()])
            .unwrap()
            .unwrap();
        assert_eq!(options.mvr.as_deref(), Some(Path::new("rig.mvr")));
    }

    #[test]
    fn product_flags_parse_in_order() {
        let options = Options::parse(vec![
            "--show".into(),
            "look.olshow".into(),
            "--desk".into(),
            "custom.oldesk".into(),
            "--quiet".into(),
            "--command".into(),
            "Fixture 1 At 50".into(),
            "--command".into(),
            "Store Cue 1".into(),
            "--script".into(),
            "-".into(),
            "--frames".into(),
            "10".into(),
            "--log".into(),
            "session.log".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(options.commands, ["Fixture 1 At 50", "Store Cue 1"]);
        assert_eq!(options.script.as_deref(), Some(Path::new("-")));
        assert_eq!(options.desk.as_deref(), Some(Path::new("custom.oldesk")));
        assert_eq!(options.log.as_deref(), Some(Path::new("session.log")));
        assert_eq!(options.frames, Some(10));
        assert!(options.quiet);
    }

    #[test]
    fn version_status_check_and_completions_parse() {
        let options = Options::parse(vec!["--version".into()]).unwrap().unwrap();
        assert!(options.version);
        let options = Options::parse(vec!["--probe".into()]).unwrap().unwrap();
        assert!(options.probe);
        let options = Options::parse(vec!["--status".into(), "--json".into()])
            .unwrap()
            .unwrap();
        assert!(options.status && options.json);
        let options = Options::parse(vec!["--check".into(), "look.olshow".into()])
            .unwrap()
            .unwrap();
        assert!(options.check);
        assert_eq!(
            options.check_path.as_deref(),
            Some(Path::new("look.olshow"))
        );
        let options = Options::parse(vec!["--check".into(), "--show".into(), "x".into()])
            .unwrap()
            .unwrap();
        assert!(options.check && options.check_path.is_none());

        assert!(Options::parse(vec!["--json".into()]).is_err());
        assert!(Options::parse(vec!["--desk".into(), "x".into()]).is_err());
        assert!(Options::parse(vec!["--check".into(), "--status".into()]).is_err());
        assert!(Options::parse(vec!["--completions".into(), "powershell".into()]).is_ok());
    }

    #[test]
    fn the_version_line_names_the_package_the_format_and_the_build() {
        let line = version_line();
        assert!(line.starts_with("openlight "), "{line}");
        assert!(line.contains(env!("CARGO_PKG_VERSION")), "{line}");
        assert!(line.contains("show format v14"), "{line}");
        if !env!("OPENLIGHT_GIT_HASH").is_empty() {
            assert!(line.contains(env!("OPENLIGHT_GIT_HASH")), "{line}");
        }
    }

    #[test]
    fn check_reports_problems_with_the_path_and_nothing_else() {
        let valid = std::env::temp_dir().join(format!(
            "openlight-console-{}-valid.olshow",
            std::process::id()
        ));
        engine::Show::new().save(&valid).unwrap();
        assert!(check_show(Some(&valid)).is_empty());
        let _ = std::fs::remove_file(&valid);

        let broken = std::env::temp_dir().join(format!(
            "openlight-console-{}-broken.olshow",
            std::process::id()
        ));
        std::fs::write(&broken, "{not json").unwrap();
        let problems = check_show(Some(&broken));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with(&format!("{}: ", broken.display())));
        let _ = std::fs::remove_file(&broken);
    }

    #[test]
    fn bad_remote_ports_and_binds_are_startup_errors() {
        assert!(Options::parse(vec!["--remote-port".into(), "nope".into()]).is_err());
        assert!(Options::parse(vec!["--remote-bind".into(), "not-an-ip".into()]).is_err());
    }

    #[test]
    fn input_bindings_use_the_output_spec_shape() {
        let options = Options::parse(vec![
            "--input".into(),
            "2=enttec:/dev/ttyUSB0".into(),
            "--input".into(),
            "1=virtual".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            options.inputs,
            vec![
                (2, "enttec:/dev/ttyUSB0".to_string()),
                (1, "virtual".to_string())
            ]
        );
        let none = Options::parse(Vec::new()).unwrap().unwrap();
        assert!(none.inputs.is_empty(), "inputs are opt-in");
    }
}

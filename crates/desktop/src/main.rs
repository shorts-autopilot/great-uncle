//! `openlight-desktop` — the windowed front end.
//!
//! The model half is always built. The window itself is behind the `gui`
//! feature so CI and the MSRV job can test the desk without a display or a GL
//! driver. `--headless-smoke` loads a show, builds a snapshot and exits before
//! any window code runs.

use std::path::PathBuf;
use std::process::ExitCode;

use openlight_app::{App, AppOptions, JournalChoice};

#[cfg(feature = "gui")]
mod gui;

const USAGE: &str = "\
openlight-desktop — OpenLight desktop window

Usage: openlight-desktop [options]

Options:
  --show <file>        load a show file at start
  --desk <file>        load this desk file instead of <show>.oldesk
  --mvr <file>         import an MVR rig before the window opens
  --gdtf <file>        import a GDTF fixture type (repeatable)
  --output <spec>      bind a universe, e.g. 1=virtual or 1=enttec:/dev/ttyUSB0
                       (repeatable, defaults to 1=virtual)
  --autosave <secs>    snapshot show and desk every <secs> (0 disables)
  --journal <file|off> journal each edit to <file> (default: <show>.journal)
  --no-recover         do not replay the journal at start
  --engine-hz <n>      engine tick rate (default 60)
  --fps <n>            output frame rate (default 30)
  --headless-smoke     build a snapshot and exit; needs no display
  --help               show this text
";

fn main() -> ExitCode {
    let args = match Args::parse(std::env::args().skip(1).collect()) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = match App::load(args.options) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    for line in app.messages() {
        println!("{line}");
    }
    for (line, outcome) in app.imports() {
        println!("> {line}");
        for line in &outcome.lines {
            println!("{line}");
        }
    }

    if args.headless_smoke {
        let snapshot = app.snapshot();
        println!(
            "OpenLight desktop {} — {} (revision {})",
            env!("CARGO_PKG_VERSION"),
            snapshot.show_name,
            snapshot.revision
        );
        println!(
            "{} sheet rows, {} cues, {} pool entries, {} timecode rows, {} universes",
            snapshot.listings.sheet.len(),
            snapshot.listings.playback.len(),
            snapshot.listings.pools.len(),
            snapshot.listings.timecode.len(),
            snapshot.frame.len()
        );
        app.stop();
        return ExitCode::SUCCESS;
    }

    #[cfg(feature = "gui")]
    {
        match gui::run(app) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        eprintln!(
            "error: this build has no window support; rebuild with `--features gui`, or use `--headless-smoke`"
        );
        app.stop();
        ExitCode::FAILURE
    }
}

struct Args {
    options: AppOptions,
    headless_smoke: bool,
}

impl Args {
    fn parse(arguments: Vec<String>) -> Result<Option<Args>, String> {
        let mut options = AppOptions::default();
        let mut headless_smoke = false;
        let mut index = 0;
        while index < arguments.len() {
            let argument = &arguments[index];
            let (flag, inline) = match argument.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_string())),
                None => (argument.as_str(), None),
            };
            match flag {
                "--show" => {
                    options.show = Some(PathBuf::from(take_value(
                        &arguments, &mut index, inline, flag,
                    )?))
                }
                "--desk" => {
                    options.desk = Some(PathBuf::from(take_value(
                        &arguments, &mut index, inline, flag,
                    )?))
                }
                "--mvr" => {
                    options.mvr = Some(PathBuf::from(take_value(
                        &arguments, &mut index, inline, flag,
                    )?))
                }
                "--gdtf" => options.gdtf.push(PathBuf::from(take_value(
                    &arguments, &mut index, inline, flag,
                )?)),
                "--output" => {
                    let spec = take_value(&arguments, &mut index, inline, flag)?;
                    options.outputs.push(parse_output(&spec)?);
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
                "--engine-hz" => {
                    options.engine_hz = take_value(&arguments, &mut index, inline, flag)?
                        .parse()
                        .map_err(|_| "--engine-hz needs a number".to_string())?;
                }
                "--fps" => {
                    options.frame_hz = take_value(&arguments, &mut index, inline, flag)?
                        .parse()
                        .map_err(|_| "--fps needs a number".to_string())?;
                }
                "--headless-smoke" => headless_smoke = true,
                "--help" | "-h" => return Ok(None),
                other => return Err(format!("unknown option `{other}`")),
            }
            index += 1;
        }
        if options.desk.is_some() && options.show.is_none() {
            return Err("--desk needs a show file".to_string());
        }
        Ok(Some(Args {
            options,
            headless_smoke,
        }))
    }
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

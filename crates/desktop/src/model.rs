//! Widget state and command mapping.
//!
//! Board keys append their text to the command line and nothing runs until
//! Please or Enter (`push_token`, then `submit`). Continuous controls — the
//! faders — send one finished line per step. Either way the text goes through
//! `App::execute`; there is no direct write to the engine, which is what stops
//! the window from becoming a second engine.

use openlight_app::{App, CommandError, CommandOutcome};

use crate::workspace::{WindowId, WindowKind, Workspace};

/// One command and the result it produced, for the Command Line History
/// window. This is UI state, not a second execution path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandHistory {
    pub command: String,
    pub message: String,
    pub error: bool,
}

/// Map a 0..=1 fader position to the whole percent a command carries.
pub fn fader_percent(position: f64) -> u8 {
    (position.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// One keyboard step on a fader: 5%, clamped to 0..=100.
pub fn step_percent(percent: f64, delta: f64) -> f64 {
    (percent + delta).clamp(0.0, 100.0)
}

/// The command a fader writes. Executor 0 is the grand master.
pub fn fader_command(executor: u16, position: f64) -> String {
    let percent = fader_percent(position);
    if executor == 0 {
        format!("Master Grand At {percent}")
    } else {
        format!("Executor {executor} At {percent}")
    }
}

/// The command a click on a cue row appends: `Go` plays the next cue of that
/// sequence; the row is a way into the list, not a jump.
pub fn cue_command(sequence: u16) -> String {
    format!("Go Sequence {sequence}")
}

/// One of `Go`, `Back`, `Pause` or `Release` for a sequence.
pub fn playback_command(sequence: u16, action: &str) -> String {
    format!("{action} Sequence {sequence}")
}

/// The command an action on a pool row writes.
pub fn pool_command(kind: &str, number: u16, detail: &str, active: bool) -> String {
    match kind {
        "Palette" => format!("At Palette {detail} {number}"),
        "Group" => format!("Group {number}"),
        "World" => {
            if active {
                "World All".to_string()
            } else {
                format!("World {number}")
            }
        }
        "Filter" => {
            if active {
                "Filter All".to_string()
            } else {
                format!("Filter {number}")
            }
        }
        "Effect" => {
            if active {
                format!("Effect {number} Off")
            } else {
                format!("Effect {number} On")
            }
        }
        other => format!("{other} {number}"),
    }
}

/// The command that removes a pool entry.
pub fn delete_command(kind: &str, number: u16, detail: &str) -> String {
    if kind == "Palette" {
        format!("Delete Palette {detail} {number}")
    } else {
        format!("Delete {kind} {number}")
    }
}

#[derive(Debug)]
pub struct Model {
    /// The user-defined screen. It starts blank.
    pub workspace: Workspace,
    pub input: String,
    /// The last outcome's lines, or the last error.
    pub message: String,
    pub error: bool,
    /// Widget actions in order, for the command line's echo.
    pub echo: Vec<String>,
    /// The sequence whose transport buttons the Sequence Sheet shows.
    pub selected_sequence: Option<u16>,
    /// Bounded command history for the Command Line History window.
    pub history: Vec<CommandHistory>,
    /// Set when an action ran; the window rebuilds its snapshot then.
    dirty: bool,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            workspace: Workspace::new(),
            input: String::new(),
            message: "Ready. Add a window or type `Help` for commands.".into(),
            error: false,
            echo: Vec::new(),
            selected_sequence: None,
            history: Vec::new(),
            dirty: false,
        }
    }
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a window to the blank screen. Layout is not a show action.
    pub fn add_window(&mut self, kind: WindowKind) -> Option<WindowId> {
        self.workspace.add(kind)
    }

    /// Add a window at a clicked cell, falling back to the first free slot.
    pub fn add_window_at(
        &mut self,
        kind: WindowKind,
        cell: Option<(u16, u16)>,
    ) -> Option<WindowId> {
        self.workspace.add_at(kind, cell)
    }

    pub fn remove_window(&mut self, id: WindowId) -> bool {
        self.workspace.remove(id)
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Run a command as if it was typed, echoing it first.
    fn act(&mut self, app: &App, command: &str) -> Result<CommandOutcome, CommandError> {
        let command = command.trim();
        if command.is_empty() {
            return Ok(CommandOutcome::default());
        }
        self.echo.push(command.to_string());
        if self.echo.len() > 50 {
            self.echo.remove(0);
        }
        match app.execute(command) {
            Ok(outcome) => {
                self.error = false;
                if outcome.lines.is_empty() {
                    self.message = format!("{command}: done");
                } else {
                    self.message = outcome.lines.join(" | ");
                }
                self.record_history(command);
                self.dirty = true;
                Ok(outcome)
            }
            Err(error) => {
                self.error = true;
                self.message = format!("error: {error}");
                self.record_history(command);
                self.dirty = true;
                Err(error)
            }
        }
    }

    fn record_history(&mut self, command: &str) {
        self.history.push(CommandHistory {
            command: command.to_string(),
            message: self.message.clone(),
            error: self.error,
        });
        if self.history.len() > 100 {
            self.history.remove(0);
        }
    }

    /// Append one key's text to the command line, the way the desk's keys do.
    /// Nothing runs until `submit` (Please or Enter).
    pub fn push_token(&mut self, token: &str) {
        let token = token.trim();
        if token.is_empty() {
            return;
        }
        if !self.input.is_empty() {
            self.input.push(' ');
        }
        self.input.push_str(token);
    }

    /// Submit the command line and clear it.
    pub fn submit(&mut self, app: &App) -> Result<CommandOutcome, CommandError> {
        let line = std::mem::take(&mut self.input);
        self.act(app, &line)
    }

    /// Choose the sequence whose transport buttons are shown, without running
    /// anything.
    pub fn select_sequence(&mut self, sequence: u16) {
        self.selected_sequence = Some(sequence);
    }

    pub fn drag_fader(
        &mut self,
        app: &App,
        executor: u16,
        position: f64,
    ) -> Result<CommandOutcome, CommandError> {
        self.act(app, &fader_command(executor, position))
    }

    /// The last echoed action, for the line above the prompt.
    pub fn last_echo(&self) -> Option<&str> {
        self.echo.last().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlight_app::{AppOptions, JournalChoice};

    fn app() -> App {
        App::load(AppOptions {
            start_runtime: false,
            journal: JournalChoice::Off,
            ..AppOptions::default()
        })
        .unwrap()
    }

    /// One patched fixture and one cue, so widget commands can succeed.
    fn app_with_cue() -> App {
        let gdtf = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gdtf/openlight-demo-wash.gdtf");
        let app = App::load(AppOptions {
            gdtf: vec![gdtf],
            start_runtime: false,
            journal: JournalChoice::Off,
            ..AppOptions::default()
        })
        .unwrap();
        app.execute("Patch Fixture 1 At 1.001 Type \"OpenLight Demo Wash\" Mode \"4ch\"")
            .unwrap();
        app.execute("Fixture 1 At 50").unwrap();
        app.execute("Store Cue 1").unwrap();
        app.execute("Assign Executor 1 Sequence 1").unwrap();
        app
    }

    #[test]
    fn a_fader_maps_to_a_whole_percent_command() {
        assert_eq!(fader_percent(0.0), 0);
        assert_eq!(fader_percent(0.634), 63);
        assert_eq!(fader_percent(0.999), 100);
        assert_eq!(fader_percent(-1.0), 0);
        assert_eq!(fader_percent(2.0), 100);
        assert_eq!(fader_command(1, 0.63), "Executor 1 At 63");
        assert_eq!(fader_command(0, 0.5), "Master Grand At 50");
    }

    #[test]
    fn a_keyboard_step_is_five_percent_and_stops_at_the_ends() {
        assert_eq!(step_percent(63.0, 5.0), 68.0);
        assert_eq!(step_percent(63.0, -5.0), 58.0);
        assert_eq!(step_percent(98.0, 5.0), 100.0);
        assert_eq!(step_percent(2.0, -5.0), 0.0);
    }

    #[test]
    fn a_cue_row_and_a_transport_key_map_to_commands() {
        assert_eq!(cue_command(1), "Go Sequence 1");
        assert_eq!(cue_command(4), "Go Sequence 4");
        assert_eq!(playback_command(1, "Back"), "Back Sequence 1");
        assert_eq!(playback_command(4, "Release"), "Release Sequence 4");
    }

    #[test]
    fn pool_rows_map_to_use_and_delete_commands() {
        assert_eq!(
            pool_command("Palette", 2, "Color", false),
            "At Palette Color 2"
        );
        assert_eq!(pool_command("Group", 3, "", false), "Group 3");
        assert_eq!(pool_command("World", 1, "", true), "World All");
        assert_eq!(pool_command("Filter", 2, "", false), "Filter 2");
        assert_eq!(pool_command("Effect", 3, "", false), "Effect 3 On");
        assert_eq!(pool_command("Effect", 3, "", true), "Effect 3 Off");
        assert_eq!(
            delete_command("Palette", 2, "Color"),
            "Delete Palette Color 2"
        );
        assert_eq!(delete_command("Group", 2, ""), "Delete Group 2");
    }

    #[test]
    fn selecting_a_sequence_runs_nothing() {
        let app = app();
        let revision = app.revision();
        let mut model = Model::new();
        model.select_sequence(4);
        assert_eq!(model.selected_sequence, Some(4));
        assert!(model.echo.is_empty());
        assert_eq!(app.revision(), revision);
        assert!(!model.take_dirty());
    }

    #[test]
    fn adding_and_removing_windows_executes_nothing() {
        let app = app();
        let revision = app.revision();
        let mut model = Model::new();
        let id = model.add_window(WindowKind::FixtureSheet).unwrap();
        assert_eq!(model.workspace.windows().len(), 1);
        assert!(model.remove_window(id));
        assert!(model.workspace.is_empty());
        assert!(model.echo.is_empty());
        assert_eq!(
            app.revision(),
            revision,
            "window layout must not touch the show"
        );
        assert!(!model.take_dirty());
    }

    #[test]
    fn command_history_records_successes_and_failures() {
        let app = app_with_cue();
        let mut model = Model::new();
        model.act(&app, "Fixture 1 At 50").unwrap();
        assert_eq!(model.history.len(), 1);
        assert_eq!(model.history[0].command, "Fixture 1 At 50");
        assert!(!model.history[0].error);

        assert!(model.act(&app, "Executor 9 At 50").is_err());
        assert_eq!(model.history.len(), 2);
        assert!(model.history[1].error);
        assert!(model.history[1].message.starts_with("error: "));
    }

    #[test]
    fn board_keys_build_one_line_and_do_not_run_it() {
        let app = app_with_cue();
        let revision = app.revision();
        let mut model = Model::new();

        model.push_token("Fixture 1");
        model.push_token(" At ");
        model.push_token("50");
        assert_eq!(model.input, "Fixture 1 At 50");
        assert!(model.echo.is_empty(), "a key must not run anything");
        assert_eq!(app.revision(), revision, "the show is untouched");

        model.submit(&app).unwrap();
        assert_eq!(model.last_echo(), Some("Fixture 1 At 50"));
        assert!(model.input.is_empty());
        assert_eq!(model.history.len(), 1);
        assert!(model.take_dirty());
    }

    #[test]
    fn an_empty_key_adds_nothing() {
        let mut model = Model::new();
        model.push_token("   ");
        assert!(model.input.is_empty());
        model.push_token(" Group ");
        assert_eq!(model.input, "Group");
    }

    #[test]
    fn a_live_fader_sends_a_finished_line() {
        let app = app_with_cue();
        let mut model = Model::new();
        model.drag_fader(&app, 1, 0.63).unwrap();
        assert_eq!(model.last_echo(), Some("Executor 1 At 63"));
        assert!(model.message.contains("Executor 1"));
        assert!(!model.error);
    }

    #[test]
    fn a_failed_widget_action_is_echoed_and_reported() {
        let app = app();
        let mut model = Model::new();
        let result = model.drag_fader(&app, 9, 0.5);
        assert!(result.is_err());
        assert_eq!(model.last_echo(), Some("Executor 9 At 50"));
        assert!(model.error);
        assert!(model.message.starts_with("error: "));
    }
}

//! Widget state and command mapping.
//!
//! Every action here ends in a command string. There is no direct write to the
//! engine, which is what stops the window from becoming a second engine.

use openlight_app::{App, CommandError, CommandOutcome, PlaybackRow};

/// The five windows. Function keys F1–F5 select them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sheet,
    Playback,
    Pools,
    Timecode,
    Status,
}

impl Pane {
    pub const ALL: [Pane; 5] = [
        Pane::Sheet,
        Pane::Playback,
        Pane::Pools,
        Pane::Timecode,
        Pane::Status,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Sheet => "Sheet",
            Pane::Playback => "Playback",
            Pane::Pools => "Pools",
            Pane::Timecode => "Timecode",
            Pane::Status => "Status",
        }
    }

    pub fn function_key(self) -> &'static str {
        match self {
            Pane::Sheet => "F1",
            Pane::Playback => "F2",
            Pane::Pools => "F3",
            Pane::Timecode => "F4",
            Pane::Status => "F5",
        }
    }

    pub fn next(self) -> Pane {
        let index = Pane::ALL
            .iter()
            .position(|pane| *pane == self)
            .unwrap_or_default();
        Pane::ALL[(index + 1) % Pane::ALL.len()]
    }
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

/// The command a click on a cue row writes: `Go` plays the next cue of that
/// sequence; the row is a way into the list, not a jump.
pub fn cue_command(row: &PlaybackRow) -> String {
    format!("Go Sequence {}", row.sequence)
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
    pub pane: Pane,
    pub input: String,
    /// The last outcome's lines, or the last error.
    pub message: String,
    pub error: bool,
    /// Widget actions in order, for the command line's echo.
    pub echo: Vec<String>,
    /// The sequence whose transport buttons the playback pane shows.
    pub selected_sequence: Option<u16>,
    /// Set when an action ran; the window rebuilds its snapshot then.
    dirty: bool,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            pane: Pane::Sheet,
            input: String::new(),
            message: "Ready. Type `Help` for commands.".into(),
            error: false,
            echo: Vec::new(),
            selected_sequence: None,
            dirty: false,
        }
    }
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    /// Switching a pane is not an action: it runs nothing.
    pub fn switch(&mut self, pane: Pane) {
        self.pane = pane;
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Run a command as if it was typed, echoing it first.
    pub fn act(&mut self, app: &App, command: &str) -> Result<CommandOutcome, CommandError> {
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
                self.dirty = true;
                Ok(outcome)
            }
            Err(error) => {
                self.error = true;
                self.message = format!("error: {error}");
                self.dirty = true;
                Err(error)
            }
        }
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

    pub fn click_cue(
        &mut self,
        app: &App,
        row: &PlaybackRow,
    ) -> Result<CommandOutcome, CommandError> {
        self.select_sequence(row.sequence);
        self.act(app, &cue_command(row))
    }

    pub fn playback(&mut self, app: &App, action: &str) -> Result<CommandOutcome, CommandError> {
        let Some(sequence) = self.selected_sequence else {
            return Ok(CommandOutcome::default());
        };
        self.act(app, &playback_command(sequence, action))
    }

    pub fn drag_fader(
        &mut self,
        app: &App,
        executor: u16,
        position: f64,
    ) -> Result<CommandOutcome, CommandError> {
        self.act(app, &fader_command(executor, position))
    }

    pub fn use_pool(
        &mut self,
        app: &App,
        kind: &str,
        number: u16,
        detail: &str,
        active: bool,
    ) -> Result<CommandOutcome, CommandError> {
        self.act(app, &pool_command(kind, number, detail, active))
    }

    pub fn delete_pool(
        &mut self,
        app: &App,
        kind: &str,
        number: u16,
        detail: &str,
    ) -> Result<CommandOutcome, CommandError> {
        self.act(app, &delete_command(kind, number, detail))
    }

    /// The last echoed action, for the line above the prompt.
    pub fn last_echo(&self) -> Option<&str> {
        self.echo.last().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlight_app::{AppOptions, JournalChoice, RowRun};

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

    fn cue_row(sequence: u16, cue: f64) -> PlaybackRow {
        PlaybackRow {
            sequence,
            sequence_name: format!("Sequence {sequence}"),
            cue,
            cue_name: String::new(),
            trigger: "Go".into(),
            fade: 0.0,
            delay: 0.0,
            mib: false,
            blocked: false,
            values: 0,
            running: Some(RowRun::default()),
        }
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
    fn clicking_a_cue_row_goes_that_sequence() {
        assert_eq!(cue_command(&cue_row(1, 2.0)), "Go Sequence 1");
        assert_eq!(cue_command(&cue_row(4, 7.5)), "Go Sequence 4");
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
    fn switching_panes_executes_nothing() {
        let app = app();
        let revision = app.revision();
        let mut model = Model::new();
        for pane in Pane::ALL {
            model.switch(pane);
            assert_eq!(model.pane, pane);
        }
        assert!(model.echo.is_empty());
        assert_eq!(
            app.revision(),
            revision,
            "switching must not touch the show"
        );
        assert!(!model.take_dirty());
    }

    #[test]
    fn every_widget_action_is_echoed_on_the_command_line() {
        let app = app_with_cue();
        let mut model = Model::new();

        model.drag_fader(&app, 1, 0.63).unwrap();
        assert_eq!(model.last_echo(), Some("Executor 1 At 63"));
        assert!(model.message.contains("Executor 1"));
        assert!(!model.error);

        let row = app.snapshot().listings.playback.remove(0);
        model.click_cue(&app, &row).unwrap();
        assert_eq!(model.last_echo(), Some("Go Sequence 1"));
        assert_eq!(model.selected_sequence, Some(1));
        model.playback(&app, "Pause").unwrap();
        assert_eq!(model.last_echo(), Some("Pause Sequence 1"));
        assert!(model.take_dirty());

        model.input = "Master Grand At 50".into();
        model.submit(&app).unwrap();
        assert_eq!(model.last_echo(), Some("Master Grand At 50"));
        assert!(model.input.is_empty());
        assert_eq!(model.echo.len(), 4);
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

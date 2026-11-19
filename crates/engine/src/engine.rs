//! Engine façade.
//!
//! `Engine` owns the show, the transient selection and the output status. Every
//! command goes through [`Engine::execute`]; mutations run inside
//! [`Engine::mutate`], which rolls back and reports if the result would not
//! validate, so a show is always safe to render and save.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::command::{self, Command, CommandError, Level};
use crate::fixture_type::DIMMER;
use crate::gdtf;
use crate::output::{lock, OutputStatus};
use crate::patch::PatchRange;
use crate::render::{self, OutputFrame};
use crate::show::{Fixture, Show};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutcome {
    pub lines: Vec<String>,
}

impl CommandOutcome {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            lines: vec![message.into()],
        }
    }

    pub fn with_lines(lines: Vec<String>) -> Self {
        Self { lines }
    }
}

pub struct Engine {
    pub show: Show,
    pub selection: Vec<u16>,
    /// Bumped on every change the output threads must pick up.
    pub revision: u64,
    pub status: Arc<Mutex<OutputStatus>>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::empty()
    }
}

impl Engine {
    pub fn new(show: Show) -> Self {
        Self {
            show,
            selection: Vec::new(),
            revision: 0,
            status: Arc::new(Mutex::new(OutputStatus::default())),
        }
    }

    pub fn empty() -> Self {
        Self::new(Show::new())
    }

    /// Run one line, possibly several `;` separated statements.
    pub fn execute(&mut self, line: &str) -> Result<CommandOutcome, CommandError> {
        let statements = command::split_statements(line);
        if statements.is_empty() {
            return Err(CommandError::new("empty command"));
        }
        let mut outcome = CommandOutcome::default();
        for statement in statements {
            let tokens = command::lex(statement)?;
            if tokens.is_empty() {
                continue;
            }
            let parsed = command::parse(&tokens)?;
            outcome.lines.extend(self.dispatch(parsed)?.lines);
        }
        Ok(outcome)
    }

    pub fn render(&self) -> OutputFrame {
        render::render(&self.show)
    }

    pub fn help_text(&self) -> Vec<String> {
        HELP.iter().map(|line| (*line).to_string()).collect()
    }

    fn dispatch(&mut self, command: Command) -> Result<CommandOutcome, CommandError> {
        match command {
            Command::Select(fixtures) => self.select(fixtures),
            Command::SetLevel { fixtures, level } => self.set_level(fixtures, level),
            Command::Off { fixtures } => self.set_level(fixtures, Level::Percent(0.0)),
            Command::Clear => {
                self.selection.clear();
                Ok(CommandOutcome::message("Selection cleared"))
            }
            Command::Patch {
                fixture,
                universe,
                address,
                type_key,
                mode,
            } => self.patch(fixture, universe, address, type_key, mode),
            Command::Unpatch { fixtures } => self.unpatch(fixtures),
            Command::ImportGdtf { path } => self.import_gdtf(&path),
            Command::Sheet => Ok(self.sheet()),
            Command::Status => Ok(self.status_report()),
            Command::Save { path } => self.save(&path),
            Command::Load { path } => self.load(&path),
            Command::Help => Ok(CommandOutcome::with_lines(self.help_text())),
        }
    }

    fn select(&mut self, fixtures: Vec<u16>) -> Result<CommandOutcome, CommandError> {
        let missing = self.missing_fixtures(&fixtures);
        if !missing.is_empty() {
            return Err(CommandError::with_hint(
                format!("fixture {} is not patched", format_fixture_list(&missing)),
                "patch it first, e.g. `Patch Fixture 1 At 1.001 Type \"…\"`",
            ));
        }
        let message = format!(
            "Selected {} ({} fixture{})",
            format_fixture_list(&fixtures),
            fixtures.len(),
            if fixtures.len() == 1 { "" } else { "s" }
        );
        self.selection = fixtures;
        Ok(CommandOutcome::message(message))
    }

    fn set_level(
        &mut self,
        fixtures: Option<Vec<u16>>,
        level: Level,
    ) -> Result<CommandOutcome, CommandError> {
        let targets = self.targets(fixtures)?;
        let value = match level {
            Level::Percent(percent) => {
                if !(0.0..=100.0).contains(&percent) {
                    return Err(CommandError::new(format!(
                        "level {percent} is outside 0..100"
                    )));
                }
                percent / 100.0
            }
            Level::Full => 1.0,
        };
        let label = format_fixture_list(&targets);
        let percent = (value * 100.0).round() as i64;

        let message = self.mutate(|show| {
            for number in &targets {
                let index = show
                    .fixture_index(*number)
                    .ok_or_else(|| CommandError::new(format!("fixture {number} is not patched")))?;
                let attribute = {
                    let fixture = &show.fixtures[index];
                    let (_, mode) = show.mode_for(fixture).map_err(CommandError::new)?;
                    mode.channels
                        .iter()
                        .find(|channel| channel.attribute.eq_ignore_ascii_case(DIMMER))
                        .map(|channel| channel.attribute.clone())
                        .ok_or_else(|| {
                            CommandError::new(format!(
                                "fixture {number} ({}) has no Dimmer channel in mode `{}`",
                                fixture.name, fixture.mode
                            ))
                        })?
                };
                show.fixtures[index].levels.insert(attribute, value);
            }
            Ok(format!("{label} {DIMMER} at {percent}%"))
        })?;
        // The fixtures you just addressed stay selected, so a following `Full`
        // or `Off` applies to the same set.
        self.selection = targets;
        Ok(CommandOutcome::message(message))
    }

    fn patch(
        &mut self,
        number: u16,
        universe: u16,
        address: u16,
        type_key: Option<String>,
        mode_name: Option<String>,
    ) -> Result<CommandOutcome, CommandError> {
        let available: Vec<String> = self
            .show
            .fixture_types
            .iter()
            .map(|fixture_type| fixture_type.key())
            .collect();
        let key = match type_key {
            Some(key) => key,
            None => match available.len() {
                0 => {
                    return Err(CommandError::with_hint(
                        "no fixture types loaded",
                        "import one first: `Import Gdtf \"file.gdtf\"`",
                    ))
                }
                1 => available[0].clone(),
                _ => {
                    return Err(CommandError::with_hint(
                        "`Type` is required when several fixture types are loaded",
                        format!("available: {}", available.join(", ")),
                    ))
                }
            },
        };

        let fixture_type = self.show.find_type(&key).cloned().ok_or_else(|| {
            CommandError::with_hint(
                format!("unknown fixture type `{key}`"),
                if available.is_empty() {
                    "import one with `Import Gdtf \"file.gdtf\"`".to_string()
                } else {
                    format!("available: {}", available.join(", "))
                },
            )
        })?;

        let mode = match mode_name {
            Some(name) => fixture_type.mode(&name).cloned().ok_or_else(|| {
                CommandError::with_hint(
                    format!("mode `{name}` is not in `{}`", fixture_type.key()),
                    format!(
                        "modes: {}",
                        fixture_type
                            .modes
                            .iter()
                            .map(|mode| mode.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?,
            None => fixture_type.default_mode().cloned().ok_or_else(|| {
                CommandError::new(format!("`{}` has no modes", fixture_type.key()))
            })?,
        };
        if mode.channels.is_empty() {
            return Err(CommandError::new(format!(
                "mode `{}` has no DMX channels",
                mode.name
            )));
        }

        let defined: Vec<String> = self
            .show
            .universes
            .iter()
            .map(|universe| universe.number.to_string())
            .collect();
        let universe_config = self.show.universe(universe).cloned().ok_or_else(|| {
            CommandError::with_hint(
                format!("universe {universe} does not exist"),
                format!("defined universes: {}", defined.join(", ")),
            )
        })?;

        let footprint = mode.footprint();
        let range = PatchRange::new(universe, address, footprint).ok_or_else(|| {
            CommandError::new(format!(
                "cannot patch {footprint} channels at {universe}.{address:03}"
            ))
        })?;
        if !range.fits(universe_config.size) {
            return Err(CommandError::new(format!(
                "fixture needs {footprint} channels at {universe}.{address:03} \
                 but universe {universe} has {} slots",
                universe_config.size
            )));
        }

        for other in &self.show.fixtures {
            if other.number == number {
                continue;
            }
            let Ok((_, other_mode)) = self.show.mode_for(other) else {
                continue;
            };
            let Some(other_range) =
                PatchRange::new(other.universe, other.address, other_mode.footprint())
            else {
                continue;
            };
            if other_range.overlaps(&range) {
                return Err(CommandError::with_hint(
                    format!(
                        "fixture {number} would overlap fixture {} at {}.{:03}",
                        other.number, other_range.universe, other_range.start
                    ),
                    "pick another address",
                ));
            }
        }

        let existed = self.show.fixture_index(number).is_some();
        let levels: BTreeMap<String, f64> = mode
            .channels
            .iter()
            .map(|channel| (channel.attribute.clone(), 0.0))
            .collect();
        let fixture = Fixture {
            number,
            name: fixture_type.name.clone(),
            type_key: fixture_type.key(),
            mode: mode.name.clone(),
            universe,
            address,
            levels,
        };
        let mode_label = mode.name.clone();

        let message = self.mutate(|show| {
            match show.fixture_index(number) {
                Some(index) => show.fixtures[index] = fixture,
                None => {
                    show.fixtures.push(fixture);
                    show.fixtures.sort_by_key(|fixture| fixture.number);
                }
            }
            Ok(format!(
                "{} fixture {number} at {universe}.{address:03} ({footprint} channels, mode `{mode_label}`)",
                if existed { "Repatched" } else { "Patched" }
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn unpatch(&mut self, fixtures: Vec<u16>) -> Result<CommandOutcome, CommandError> {
        let selected = fixtures.clone();
        let message = self.mutate(|show| {
            let missing: Vec<u16> = selected
                .iter()
                .copied()
                .filter(|number| show.fixture_index(*number).is_none())
                .collect();
            if !missing.is_empty() {
                return Err(CommandError::new(format!(
                    "fixture {} is not patched",
                    format_fixture_list(&missing)
                )));
            }
            show.fixtures
                .retain(|fixture| !selected.contains(&fixture.number));
            Ok(format!("Unpatched {}", format_fixture_list(&selected)))
        })?;
        self.selection.retain(|number| !fixtures.contains(number));
        Ok(CommandOutcome::message(message))
    }

    fn import_gdtf(&mut self, path: &str) -> Result<CommandOutcome, CommandError> {
        let report =
            gdtf::import(Path::new(path)).map_err(|error| CommandError::new(error.to_string()))?;
        let types = report.types;
        let mut added = 0usize;
        let mut replaced = 0usize;

        let message = self.mutate(|show| {
            for fixture_type in &types {
                let key = fixture_type.key();
                match show
                    .fixture_types
                    .iter()
                    .position(|existing| existing.key().eq_ignore_ascii_case(&key))
                {
                    Some(index) => {
                        if show.fixture_types[index] != *fixture_type {
                            show.fixture_types[index] = fixture_type.clone();
                            replaced += 1;
                        }
                    }
                    None => {
                        show.fixture_types.push(fixture_type.clone());
                        added += 1;
                    }
                }
            }
            Ok(format!(
                "Imported {added} new fixture type{}, replaced {replaced}",
                if added == 1 { "" } else { "s" }
            ))
        })?;

        let mut lines = vec![message];
        lines.extend(
            report
                .warnings
                .into_iter()
                .map(|warning| format!("Warning: {warning}")),
        );
        Ok(CommandOutcome::with_lines(lines))
    }

    fn save(&self, path: &str) -> Result<CommandOutcome, CommandError> {
        self.show
            .save(Path::new(path))
            .map_err(|error| CommandError::new(error.to_string()))?;
        Ok(CommandOutcome::message(format!("Saved show to {path}")))
    }

    fn load(&mut self, path: &str) -> Result<CommandOutcome, CommandError> {
        let show =
            Show::load(Path::new(path)).map_err(|error| CommandError::new(error.to_string()))?;
        let message = format!(
            "Loaded `{}` ({} fixtures, {} fixture types) from {path}",
            show.name,
            show.fixtures.len(),
            show.fixture_types.len()
        );
        self.show = show;
        self.selection.clear();
        self.revision += 1;
        Ok(CommandOutcome::message(message))
    }

    fn sheet(&self) -> CommandOutcome {
        if self.show.fixtures.is_empty() {
            return CommandOutcome::message("No fixtures patched.");
        }
        let mut lines = vec![format!(
            "{:<4} {:<22} {:<26} {:<10} {:<10} {}",
            "No.", "Name", "Type", "Mode", "Patch", "Levels"
        )];
        let mut fixtures: Vec<&Fixture> = self.show.fixtures.iter().collect();
        fixtures.sort_by_key(|fixture| fixture.number);

        for fixture in fixtures {
            let (patch, levels) = match self.show.mode_for(fixture) {
                Ok((fixture_type, mode)) => {
                    let mut seen = BTreeSet::new();
                    let mut parts = Vec::new();
                    for channel in &mode.channels {
                        if !seen.insert(channel.attribute.to_ascii_lowercase()) {
                            continue;
                        }
                        let level = fixture
                            .levels
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(&channel.attribute))
                            .map(|(_, level)| *level)
                            .unwrap_or(0.0);
                        parts.push(format!(
                            "{} {}%",
                            fixture_type.pretty_attribute(&channel.attribute),
                            (level * 100.0).round() as i64
                        ));
                    }
                    (
                        format!("{}.{:03}", fixture.universe, fixture.address),
                        parts.join("  "),
                    )
                }
                Err(error) => ("?".to_string(), error),
            };
            lines.push(format!(
                "{:<4} {:<22} {:<26} {:<10} {:<10} {}",
                fixture.number,
                truncate(&fixture.name, 22),
                truncate(&fixture.type_key, 26),
                truncate(&fixture.mode, 10),
                patch,
                levels
            ));
        }
        CommandOutcome::with_lines(lines)
    }

    fn status_report(&self) -> CommandOutcome {
        let status = lock(&self.status);
        let mut lines = vec![
            format!(
                "Output: {} — {} Hz frames, {} Hz engine tick",
                if status.running { "running" } else { "stopped" },
                status.frame_hz,
                status.engine_hz
            ),
            format!("Frames sent: {}", status.frames),
        ];
        if status.universes.is_empty() {
            lines.push(
                "No output transports bound; start the console with `--output <spec>`.".into(),
            );
        }
        for (universe, entry) in &status.universes {
            let last_error = entry
                .last_error
                .as_ref()
                .map(|error| format!(" (last error: {error})"))
                .unwrap_or_default();
            lines.push(format!(
                "Universe {universe}: {} — {} frames, {} errors{}",
                entry.transport, entry.frames, entry.errors, last_error
            ));
        }
        for warning in status.warnings.iter().take(8) {
            lines.push(format!("Warning: {warning}"));
        }
        if status.warnings.len() > 8 {
            lines.push(format!("… {} more warnings", status.warnings.len() - 8));
        }
        lines.push(format!(
            "Show: {} — {} universes, {} fixture types, {} fixtures",
            self.show.name,
            self.show.universes.len(),
            self.show.fixture_types.len(),
            self.show.fixtures.len()
        ));
        lines.push(format!(
            "Selection: {}",
            if self.selection.is_empty() {
                "none".to_string()
            } else {
                format_fixture_list(&self.selection)
            }
        ));
        CommandOutcome::with_lines(lines)
    }

    fn targets(&self, fixtures: Option<Vec<u16>>) -> Result<Vec<u16>, CommandError> {
        let list = fixtures.unwrap_or_else(|| self.selection.clone());
        if list.is_empty() {
            return Err(CommandError::with_hint(
                "nothing selected",
                "select fixtures first, e.g. `Fixture 1 Thru 4`, or name them: `Fixture 3 At 50`",
            ));
        }
        let missing = self.missing_fixtures(&list);
        if !missing.is_empty() {
            return Err(CommandError::new(format!(
                "fixture {} is not patched",
                format_fixture_list(&missing)
            )));
        }
        Ok(list)
    }

    fn missing_fixtures(&self, fixtures: &[u16]) -> Vec<u16> {
        let mut missing: Vec<u16> = fixtures
            .iter()
            .copied()
            .filter(|number| self.show.fixture_index(*number).is_none())
            .collect();
        missing.sort_unstable();
        missing.dedup();
        missing
    }

    /// Apply a change only if the resulting show still validates.
    fn mutate<F>(&mut self, change: F) -> Result<String, CommandError>
    where
        F: FnOnce(&mut Show) -> Result<String, CommandError>,
    {
        let backup = self.show.clone();
        let message = match change(&mut self.show) {
            Ok(message) => message,
            Err(error) => {
                self.show = backup;
                return Err(error);
            }
        };
        let problems = self.show.validate();
        if !problems.is_empty() {
            self.show = backup;
            return Err(CommandError::new(format!(
                "command would leave the show invalid: {}",
                problems.join("; ")
            )));
        }
        self.revision += 1;
        Ok(message)
    }
}

pub const HELP: &[&str] = &[
    "Fixture 1 Thru 5                       select fixtures",
    "Fixture 1 Thru 5 At 50                 set Dimmer (0 to 100)",
    "At 50 / Full / Off                     apply to the current selection",
    "Clear                                  clear the selection",
    "Patch Fixture 1 At 1.001 Type \"<fixture type>\" [Mode \"<mode>\"]",
    "Unpatch Fixture 1 Thru 3",
    "Import Gdtf \"file.gdtf\"                load a GDTF fixture type",
    "Sheet                                  print the fixture sheet",
    "Status                                 output and show state",
    "Save \"show.json\" / Load \"show.json\"",
    "Help                                   this list",
    "Separate several commands with `;`.",
];

/// Compact fixture list: `1 Thru 3 + 7`.
pub fn format_fixture_list(fixtures: &[u16]) -> String {
    let mut numbers = fixtures.to_vec();
    numbers.sort_unstable();
    numbers.dedup();
    if numbers.is_empty() {
        return "none".to_string();
    }

    let mut runs: Vec<(u16, u16)> = Vec::new();
    let mut start = numbers[0];
    let mut previous = numbers[0];
    for number in numbers.iter().skip(1) {
        if *number == previous + 1 {
            previous = *number;
            continue;
        }
        runs.push((start, previous));
        start = *number;
        previous = *number;
    }
    runs.push((start, previous));

    runs.iter()
        .map(|(start, end)| {
            if start == end {
                start.to_string()
            } else if *end == *start + 1 {
                format!("{start} + {end}")
            } else {
                format!("{start} Thru {end}")
            }
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut result: String = text.chars().take(width.saturating_sub(1)).collect();
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{demo_fixture_type, demo_show};

    fn engine() -> Engine {
        Engine::new(demo_show())
    }

    fn dimmer(engine: &Engine, fixture: u16) -> f64 {
        *engine
            .show
            .fixture(fixture)
            .unwrap()
            .levels
            .get("Dimmer")
            .unwrap()
    }

    #[test]
    fn formats_fixture_lists_compactly() {
        assert_eq!(format_fixture_list(&[1, 2, 3, 4]), "1 Thru 4");
        assert_eq!(format_fixture_list(&[1, 2]), "1 + 2");
        assert_eq!(format_fixture_list(&[1, 3, 4, 5, 9]), "1 + 3 Thru 5 + 9");
        assert_eq!(format_fixture_list(&[]), "none");
    }

    #[test]
    fn at_and_full_drive_the_rendered_frame() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 50").unwrap();
        assert_eq!(dimmer(&engine, 1), 0.5);
        assert_eq!(engine.render().slot(1, 1), Some(128));

        engine.execute("Full").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(255));
    }

    #[test]
    fn off_returns_to_zero() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 75").unwrap();
        engine.execute("Off").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(0));
    }

    #[test]
    fn selection_persists_between_commands() {
        let mut engine = engine();
        engine.execute("Fixture 1").unwrap();
        engine.execute("At 25").unwrap();
        assert_eq!(dimmer(&engine, 1), 0.25);
        engine.execute("Clear").unwrap();
        let error = engine.execute("At 25").unwrap_err();
        assert!(error.hint.unwrap().contains("select fixtures"));
    }

    #[test]
    fn at_without_selection_is_rejected_by_the_engine() {
        let mut engine = engine();
        let error = engine.execute("At 25").unwrap_err();
        assert_eq!(error.message, "nothing selected");
    }

    #[test]
    fn levels_outside_zero_to_hundred_are_rejected() {
        let mut engine = engine();
        engine.execute("Fixture 1").unwrap();
        assert!(engine.execute("At 101").is_err());
        assert_eq!(dimmer(&engine, 1), 0.0, "rejected command changed nothing");
    }

    #[test]
    fn thru_selects_a_range_for_the_next_level() {
        let mut engine = engine();
        engine
            .execute(r#"Patch Fixture 2 At 1.010 Type "OpenLight Demo Wash""#)
            .unwrap();
        engine.execute("Fixture 1 Thru 2 At 40").unwrap();
        assert_eq!(dimmer(&engine, 1), 0.4);
        assert_eq!(dimmer(&engine, 2), 0.4);
        assert_eq!(engine.render().slot(1, 10), Some(102));
    }

    #[test]
    fn multiple_statements_run_in_order() {
        let mut engine = engine();
        let outcome = engine.execute("Fixture 1 At 10; Fixture 1 At 90").unwrap();
        assert_eq!(outcome.lines.len(), 2);
        assert_eq!(dimmer(&engine, 1), 0.9);
    }

    #[test]
    fn patch_creates_a_fixture_with_canonical_levels() {
        let mut engine = Engine::empty();
        engine.show.fixture_types.push(demo_fixture_type());
        let outcome = engine
            .execute(r#"Patch Fixture 3 At 1.101 Type "OpenLight Demo Wash" Mode "4ch""#)
            .unwrap();
        assert!(outcome.lines[0].contains("Patched fixture 3"));
        let fixture = engine.show.fixture(3).unwrap();
        assert_eq!(fixture.address, 101);
        assert_eq!(fixture.name, "Demo Wash");
        assert_eq!(fixture.levels.len(), 4);
        assert!(fixture.levels.values().all(|level| *level == 0.0));
    }

    #[test]
    fn patch_rejects_overlaps_and_overruns() {
        let mut engine = engine();
        engine.show.fixture_types.push(demo_fixture_type());
        let overlap = engine
            .execute(r#"Patch Fixture 2 At 1.003 Type "OpenLight Demo Wash""#)
            .unwrap_err();
        assert!(overlap.message.contains("overlap"), "{overlap}");
        let overrun = engine
            .execute(r#"Patch Fixture 2 At 1.510 Type "OpenLight Demo Wash""#)
            .unwrap_err();
        assert!(overrun.message.contains("510"), "{overrun}");
        assert_eq!(engine.show.fixtures.len(), 1, "nothing was added");
    }

    #[test]
    fn patch_reports_unknown_types_and_modes() {
        let mut engine = engine();
        let unknown_type = engine
            .execute(r#"Patch Fixture 2 At 1.010 Type "Nope""#)
            .unwrap_err();
        assert!(unknown_type.hint.unwrap().contains("OpenLight Demo Wash"));
        let unknown_mode = engine
            .execute(r#"Patch Fixture 2 At 1.010 Type "OpenLight Demo Wash" Mode "99ch""#)
            .unwrap_err();
        assert!(unknown_mode.hint.unwrap().contains("4ch"));
    }

    #[test]
    fn repatch_keeps_fixture_number_and_updates_address() {
        let mut engine = engine();
        engine
            .execute(r#"Patch Fixture 1 At 1.020 Type "OpenLight Demo Wash""#)
            .unwrap();
        assert_eq!(engine.show.fixtures.len(), 1);
        assert_eq!(engine.show.fixture(1).unwrap().address, 20);
    }

    #[test]
    fn unpatch_removes_fixture_and_prunes_selection() {
        let mut engine = engine();
        engine.execute("Fixture 1").unwrap();
        engine.execute("Unpatch Fixture 1").unwrap();
        assert!(engine.show.fixtures.is_empty());
        assert!(engine.selection.is_empty());
        let error = engine.execute("Full").unwrap_err();
        assert_eq!(error.message, "nothing selected");
    }

    #[test]
    fn fixture_without_dimmer_is_reported() {
        let mut show = Show::new();
        let mut fixture_type = demo_fixture_type();
        fixture_type.modes[0]
            .channels
            .retain(|channel| channel.attribute == "ColorAdd_R");
        let type_key = fixture_type.key();
        show.fixture_types.push(fixture_type);
        show.fixtures.push(Fixture {
            number: 1,
            name: "Demo".into(),
            type_key,
            mode: "4ch".into(),
            universe: 1,
            address: 1,
            levels: BTreeMap::from([("ColorAdd_R".into(), 0.0)]),
        });
        let mut engine = Engine::new(show);
        let error = engine.execute("Fixture 1 At 50").unwrap_err();
        assert!(error.message.contains("no Dimmer channel"), "{error}");
    }

    #[test]
    fn sheet_lists_patch_and_levels() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 50").unwrap();
        let sheet = engine.execute("Sheet").unwrap();
        assert!(sheet.lines[0].contains("No."));
        assert!(sheet.lines[1].contains("1.001"), "{:?}", sheet.lines);
        assert!(sheet.lines[1].contains("Dimmer 50%"), "{:?}", sheet.lines);
    }

    #[test]
    fn sheet_with_no_fixtures_says_so() {
        let engine = Engine::empty();
        assert_eq!(engine.sheet().lines, vec!["No fixtures patched."]);
    }

    #[test]
    fn status_reports_show_state_and_selection() {
        let mut engine = engine();
        engine.execute("Fixture 1").unwrap();
        let status = engine.execute("Status").unwrap();
        assert!(status.lines.iter().any(|line| line.contains("stopped")));
        assert!(status.lines.iter().any(|line| line.contains("1 fixture")));
        assert!(status
            .lines
            .iter()
            .any(|line| line.contains("Selection: 1")));
    }

    #[test]
    fn save_and_load_roundtrip_through_commands() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 33").unwrap();
        let path = std::env::temp_dir().join(format!(
            "openlight-engine-{}-roundtrip.olshow.json",
            std::process::id()
        ));
        engine
            .execute(&format!("Save \"{}\"", path.display()))
            .unwrap();

        let mut other = Engine::empty();
        other
            .execute(&format!("Load \"{}\"", path.display()))
            .unwrap();
        assert_eq!(other.show, engine.show);
        assert_eq!(other.render().slot(1, 1), Some(84));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn failed_load_leaves_the_show_untouched() {
        let mut engine = engine();
        let before = engine.show.clone();
        assert!(engine.execute("Load \"/nonexistent/show.json\"").is_err());
        assert_eq!(engine.show, before);
    }

    #[test]
    fn import_gdtf_adds_types_and_reports_warnings() {
        let mut engine = Engine::empty();
        let path = std::env::temp_dir().join(format!(
            "openlight-engine-{}-import.gdtf",
            std::process::id()
        ));
        write_demo_gdtf(&path);
        let outcome = engine
            .execute(&format!("Import Gdtf \"{}\"", path.display()))
            .unwrap();
        assert!(
            outcome.lines[0].contains("1 new fixture type"),
            "{outcome:?}"
        );
        assert_eq!(engine.show.fixture_types.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn help_lists_the_core_commands() {
        let engine = Engine::empty();
        let help = engine.help_text().join("\n");
        for keyword in [
            "Patch", "At", "Full", "Off", "Sheet", "Status", "Save", "Load",
        ] {
            assert!(help.contains(keyword), "help mentions {keyword}");
        }
    }

    fn write_demo_gdtf(path: &Path) {
        use std::io::Write as _;
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        writer
            .start_file("description.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(crate::gdtf::tests::DEMO_XML.as_bytes())
            .unwrap();
        writer.finish().unwrap();
    }
}

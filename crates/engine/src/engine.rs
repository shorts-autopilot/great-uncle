//! Engine façade.
//!
//! `Engine` owns the show, the transient selection and the output status. Every
//! command goes through [`Engine::execute`]; mutations run inside
//! [`Engine::mutate`], which rolls back and reports if the result would not
//! validate, so a show is always safe to render and save.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::command::{self, Command, CommandError, Level, LevelSource, PlaybackRef};
use crate::fixture_type::DIMMER;
use crate::gdtf;
use crate::levels::Levels;
use crate::output::{lock, OutputStatus};
use crate::palette::{self, Palette};
use crate::patch::PatchRange;
use crate::playback::{format_cue_number, Executor, Playback, Sequence, Target, MAX_TIME};
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
    /// Running cues and fader masters. Not part of the show file.
    pub playback: Playback,
    /// The instant the latest [`Engine::tick`] saw. Commands that start a fade
    /// use it, so a test can drive time explicitly.
    clock: Instant,
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
            playback: Playback::new(),
            clock: Instant::now(),
        }
    }

    /// The time an upcoming command will use to start a fade.
    pub fn clock(&self) -> Instant {
        self.clock
    }

    /// Advance playback to `now`. Returns true when the output changed, which
    /// is what makes the output thread re-render and send.
    pub fn tick(&mut self, now: Instant) -> bool {
        let now = if now > self.clock { now } else { self.clock };
        self.clock = now;
        let changed = self.playback.refresh(&self.show, now);
        if changed {
            self.revision += 1;
        }
        changed
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
        render::render(&self.show, self.playback.output())
    }

    pub fn help_text(&self) -> Vec<String> {
        HELP.iter().map(|line| (*line).to_string()).collect()
    }

    fn dispatch(&mut self, command: Command) -> Result<CommandOutcome, CommandError> {
        match command {
            Command::Select(fixtures) => self.select(fixtures),
            Command::SetLevel {
                fixtures,
                attribute,
                source,
            } => self.set_level(fixtures, attribute, source),
            Command::Off { fixtures } => {
                self.set_level(fixtures, None, LevelSource::Level(Level::Percent(0.0)))
            }
            Command::Clear { all, values } => self.clear(all, values),
            Command::Patch {
                fixture,
                universe,
                address,
                type_key,
                mode,
            } => self.patch(fixture, universe, address, type_key, mode),
            Command::Unpatch { fixtures } => self.unpatch(fixtures),
            Command::ImportGdtf { path } => self.import_gdtf(&path),
            Command::New { name } => self.new_show(name),
            Command::StorePalette {
                feature,
                number,
                name,
            } => self.store_palette(&feature, number, name),
            Command::DeletePalette { feature, number } => self.delete_palette(&feature, number),
            Command::ListPalettes => Ok(self.palette_list()),
            Command::StoreCue {
                sequence,
                number,
                name,
                fade,
                delay,
            } => self.store_cue(sequence, number, name, fade, delay),
            Command::DeleteCue { sequence, number } => self.delete_cue(sequence, number),
            Command::ListSequences { only } => Ok(self.sequence_list(only)),
            Command::AssignExecutor { executor, sequence } => {
                self.assign_executor(executor, sequence)
            }
            Command::SetExecutorLevel { executor, level } => {
                self.set_executor_level(executor, level)
            }
            Command::ListExecutors { only } => Ok(self.executor_list(only)),
            Command::Go { target } => self.playback_go(target),
            Command::Back { target } => self.playback_back(target),
            Command::Pause { target } => self.playback_pause(target),
            Command::Release { target } => self.playback_release(target),
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
        attribute: Option<String>,
        source: LevelSource,
    ) -> Result<CommandOutcome, CommandError> {
        let targets = self.targets(fixtures)?;
        let level = match source {
            LevelSource::Level(level) => level,
            LevelSource::Palette { feature, number } => {
                if attribute.is_some() {
                    return Err(CommandError::with_hint(
                        "a palette cannot be combined with an attribute",
                        "use `At ColorAdd_R 50` or `At Palette Color 1`, not both",
                    ));
                }
                let outcome = self.apply_palette(&targets, &feature, number)?;
                self.selection = targets;
                return Ok(outcome);
            }
        };

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
        let attribute = match attribute {
            Some(name) => self.resolve_attribute(&targets, &name)?,
            None => self.dimmer_attribute(&targets)?,
        };

        for number in &targets {
            let fixture = self
                .show
                .fixture(*number)
                .ok_or_else(|| CommandError::new(format!("fixture {number} is not patched")))?;
            let (_, mode) = self.show.mode_for(fixture).map_err(CommandError::new)?;
            if mode.channel_for(&attribute).is_none() {
                return Err(CommandError::with_hint(
                    format!(
                        "fixture {number} has no `{attribute}` in mode `{}`",
                        fixture.mode
                    ),
                    format!("attributes: {}", attribute_list(mode)),
                ));
            }
        }

        let label = format_fixture_list(&targets);
        let percent = (value * 100.0).round() as i64;
        let canonical = attribute.clone();

        let message = self.mutate(|show| {
            for number in &targets {
                let index = show
                    .fixture_index(*number)
                    .ok_or_else(|| CommandError::new(format!("fixture {number} is not patched")))?;
                let key = {
                    let fixture = &show.fixtures[index];
                    let (_, mode) = show.mode_for(fixture).map_err(CommandError::new)?;
                    mode.channel_for(&canonical)
                        .map(|channel| channel.attribute.clone())
                        .ok_or_else(|| {
                            CommandError::new(format!(
                                "fixture {number} has no `{canonical}` in mode `{}`",
                                fixture.mode
                            ))
                        })?
                };
                show.fixtures[index].levels.insert(key, value);
            }
            Ok(format!("{label} {canonical} at {percent}%"))
        })?;
        // The fixtures you just addressed stay selected, so a following `Full`
        // or `Off` applies to the same set.
        self.selection = targets;
        Ok(CommandOutcome::message(message))
    }

    /// Canonical Dimmer attribute, refusing fixtures that have none.
    fn dimmer_attribute(&self, targets: &[u16]) -> Result<String, CommandError> {
        let Some(number) = targets.first() else {
            return Ok(DIMMER.to_string());
        };
        let fixture = self
            .show
            .fixture(*number)
            .ok_or_else(|| CommandError::new(format!("fixture {number} is not patched")))?;
        let (_, mode) = self.show.mode_for(fixture).map_err(CommandError::new)?;
        match mode.channel_for(DIMMER) {
            Some(channel) => Ok(channel.attribute.clone()),
            None => Err(CommandError::with_hint(
                format!(
                    "fixture {number} ({}) has no Dimmer channel in mode `{}`",
                    fixture.name, fixture.mode
                ),
                format!("attributes: {}", attribute_list(mode)),
            )),
        }
    }

    /// Resolve an attribute or pretty name against the selection.
    fn resolve_attribute(&self, targets: &[u16], requested: &str) -> Result<String, CommandError> {
        let mut available: BTreeSet<String> = BTreeSet::new();
        for number in targets {
            let Some(fixture) = self.show.fixture(*number) else {
                continue;
            };
            let (fixture_type, mode) = self.show.mode_for(fixture).map_err(CommandError::new)?;
            for channel in &mode.channels {
                available.insert(fixture_type.pretty_attribute(&channel.attribute));
            }
            if let Some(channel) = mode.channel_for(requested) {
                return Ok(channel.attribute.clone());
            }
            if let Some(channel) = mode.channels.iter().find(|channel| {
                fixture_type
                    .pretty_attribute(&channel.attribute)
                    .eq_ignore_ascii_case(requested)
            }) {
                return Ok(channel.attribute.clone());
            }
        }
        Err(CommandError::with_hint(
            format!("no attribute `{requested}` in the selection"),
            if available.is_empty() {
                "patch fixtures first".to_string()
            } else {
                format!(
                    "attributes: {}",
                    available.into_iter().collect::<Vec<_>>().join(", ")
                )
            },
        ))
    }

    fn clear(&mut self, all: bool, values: bool) -> Result<CommandOutcome, CommandError> {
        if !values {
            self.selection.clear();
            return Ok(CommandOutcome::message("Selection cleared"));
        }
        let targets: Vec<u16> = if all {
            self.show
                .fixtures
                .iter()
                .map(|fixture| fixture.number)
                .collect()
        } else {
            self.targets(None)?
        };
        let label = if all {
            "all fixtures".to_string()
        } else {
            format_fixture_list(&targets)
        };
        let mut cleared = 0usize;
        let message = self.mutate(|show| {
            for number in &targets {
                if let Some(index) = show.fixture_index(*number) {
                    cleared += show.fixtures[index].levels.len();
                    show.fixtures[index].levels.clear();
                }
            }
            Ok(format!(
                "Cleared {cleared} programmer value{} on {label}",
                if cleared == 1 { "" } else { "s" }
            ))
        })?;
        if all {
            self.selection.clear();
        }
        Ok(CommandOutcome::message(message))
    }

    fn new_show(&mut self, name: Option<String>) -> Result<CommandOutcome, CommandError> {
        let mut show = Show::new();
        if let Some(name) = name {
            if name.trim().is_empty() {
                return Err(CommandError::new("new show name is empty"));
            }
            show.name = name;
        }
        let message = format!("New show `{}`", show.name);
        self.show = show;
        self.selection.clear();
        self.playback.clear();
        self.revision += 1;
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
        // Patching starts dark: a fresh fixture has an empty programmer, not a
        // map of zeroes, so playback is free to drive it immediately.
        let fixture = Fixture {
            number,
            name: fixture_type.name.clone(),
            type_key: fixture_type.key(),
            mode: mode.name.clone(),
            universe,
            address,
            levels: Default::default(),
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
        // A loaded show has its own cues; a run from the previous show must not
        // keep writing the old values over the new rig.
        self.playback.clear();
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
        lines.push(format!(
            "Palettes: {} | Sequences: {} | Executors: {}",
            self.show.palettes.len(),
            self.show.sequences.len(),
            self.show.executors.len()
        ));
        if self.playback.is_running() {
            for (target, run) in self.playback.runs() {
                let state = if run.is_paused() {
                    "paused".to_string()
                } else if run.done {
                    "complete".to_string()
                } else {
                    format!("{:.0}%", run.progress(self.clock) * 100.0)
                };
                lines.push(format!(
                    "Playback {}: {} ({state})",
                    target.label(),
                    self.running_cue_label(target)
                ));
            }
        } else {
            lines.push("Playback: idle".to_string());
        }
        CommandOutcome::with_lines(lines)
    }

    /// Fixtures a store captures: the selection, or every fixture when nothing
    /// is selected.
    fn programmer_targets(&self) -> Result<Vec<u16>, CommandError> {
        if self.selection.is_empty() {
            let all: Vec<u16> = self
                .show
                .fixtures
                .iter()
                .map(|fixture| fixture.number)
                .collect();
            if all.is_empty() {
                return Err(CommandError::with_hint(
                    "nothing to store",
                    "patch a fixture first: `Patch Fixture 1 At 1.001 Type \"…\"`",
                ));
            }
            Ok(all)
        } else {
            self.targets(None)
        }
    }

    fn store_palette(
        &mut self,
        feature: &str,
        number: u16,
        name: Option<String>,
    ) -> Result<CommandOutcome, CommandError> {
        if number == 0 {
            return Err(CommandError::new("palette number 0 is not allowed"));
        }
        let targets = self.programmer_targets()?;
        let (pool, values) =
            palette::capture(&self.show, &targets, feature).map_err(CommandError::new)?;
        let value_count: usize = values.values().map(|values| values.len()).sum();
        let type_count = values.len();
        let name_label = name
            .as_ref()
            .map(|name| format!(" \"{name}\""))
            .unwrap_or_default();
        let mut created = false;

        let message = self.mutate(|show| {
            match show.palettes.iter_mut().find(|palette| {
                palette.number == number && palette.feature.eq_ignore_ascii_case(&pool)
            }) {
                Some(palette) => {
                    palette.values = values.clone();
                    if let Some(name) = &name {
                        palette.name = name.clone();
                    }
                }
                None => {
                    created = true;
                    show.palettes.push(Palette {
                        feature: pool.clone(),
                        number,
                        name: name.clone().unwrap_or_default(),
                        values: values.clone(),
                    });
                }
            }
            show.palettes.sort_by(|left, right| {
                left.feature
                    .to_ascii_lowercase()
                    .cmp(&right.feature.to_ascii_lowercase())
                    .then(left.number.cmp(&right.number))
            });
            Ok(format!(
                "{} palette {pool} {number}{name_label} ({type_count} fixture type{}, {value_count} values)",
                if created { "Stored" } else { "Updated" },
                if type_count == 1 { "" } else { "s" }
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn apply_palette(
        &mut self,
        targets: &[u16],
        feature: &str,
        number: u16,
    ) -> Result<CommandOutcome, CommandError> {
        let Some(palette) = self.show.palette(feature, number).cloned() else {
            let available: Vec<String> = self
                .show
                .palettes
                .iter()
                .filter(|palette| palette.feature.eq_ignore_ascii_case(feature.trim()))
                .map(|palette| palette.number.to_string())
                .collect();
            return Err(CommandError::with_hint(
                format!("palette {feature} {number} does not exist"),
                if available.is_empty() {
                    format!("store one first: `Store Palette {feature} {number}`")
                } else {
                    format!("stored {feature} palettes: {}", available.join(", "))
                },
            ));
        };

        let mut assignments = Vec::new();
        let mut skipped = Vec::new();
        for target in targets {
            let Some(fixture) = self.show.fixture(*target) else {
                continue;
            };
            let values = palette::resolve(&palette, &self.show, fixture);
            if values.is_empty() {
                skipped.push(*target);
            } else {
                assignments.push((*target, values));
            }
        }

        if assignments.is_empty() {
            return Err(CommandError::with_hint(
                format!(
                    "palette {} has no values for the selection",
                    palette.display_name()
                ),
                "palettes store values per fixture type; store this one from a fixture of this type",
            ));
        }

        let applied = assignments.len();
        let value_count: usize = assignments.iter().map(|(_, values)| values.len()).sum();
        let label = palette.display_name();
        let message = self.mutate(|show| {
            for (number, values) in &assignments {
                let Some(index) = show.fixture_index(*number) else {
                    continue;
                };
                for (attribute, level) in values {
                    show.fixtures[index]
                        .levels
                        .insert(attribute.clone(), *level);
                }
            }
            Ok(format!(
                "Applied palette {label} to {applied} fixture{} ({value_count} values)",
                if applied == 1 { "" } else { "s" }
            ))
        })?;

        let mut lines = vec![message];
        if !skipped.is_empty() {
            lines.push(format!(
                "Skipped {}: no values stored for that fixture type",
                format_fixture_list(&skipped)
            ));
        }
        Ok(CommandOutcome::with_lines(lines))
    }

    fn delete_palette(
        &mut self,
        feature: &str,
        number: u16,
    ) -> Result<CommandOutcome, CommandError> {
        if self.show.palette(feature, number).is_none() {
            return Err(CommandError::with_hint(
                format!("palette {feature} {number} does not exist"),
                "`Palette` lists the stored palettes",
            ));
        }
        let message = self.mutate(|show| {
            show.palettes.retain(|palette| {
                !(palette.number == number && palette.feature.eq_ignore_ascii_case(feature.trim()))
            });
            Ok(format!("Deleted palette {feature} {number}"))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn palette_list(&self) -> CommandOutcome {
        if self.show.palettes.is_empty() {
            return CommandOutcome::message(
                "No palettes stored. Select fixtures and run `Store Palette Color 1 \"Warm\"`.",
            );
        }
        let mut lines = vec![format!(
            "{:<10} {:<4} {:<20} {}",
            "Feature", "No.", "Name", "Values"
        )];
        for palette in &self.show.palettes {
            let name = if palette.name.trim().is_empty() {
                "-".to_string()
            } else {
                truncate(&palette.name, 20)
            };
            let summary = palette
                .values
                .iter()
                .map(|(type_key, values)| {
                    let mut parts: Vec<String> = values
                        .iter()
                        .map(|(attribute, level)| {
                            format!("{attribute} {}%", (level * 100.0).round() as i64)
                        })
                        .collect();
                    parts.sort();
                    format!("{type_key}: {}", parts.join(" "))
                })
                .collect::<Vec<_>>()
                .join(" | ");
            lines.push(format!(
                "{:<10} {:<4} {:<20} {summary}",
                palette.feature, palette.number, name
            ));
        }
        CommandOutcome::with_lines(lines)
    }

    fn store_cue(
        &mut self,
        sequence_number: u16,
        number: f64,
        name: Option<String>,
        fade: Option<f64>,
        delay: Option<f64>,
    ) -> Result<CommandOutcome, CommandError> {
        if sequence_number == 0 {
            return Err(CommandError::new("sequence number 0 is not allowed"));
        }
        if !number.is_finite() || number < 0.0 {
            return Err(CommandError::new(format!(
                "cue number {number} is not a finite positive number"
            )));
        }
        for (label, value) in [("fade", fade), ("delay", delay)] {
            if let Some(value) = value {
                if !value.is_finite() || !(0.0..=MAX_TIME).contains(&value) {
                    return Err(CommandError::new(format!(
                        "{label} {value} is outside 0..={MAX_TIME} seconds"
                    )));
                }
            }
        }

        let targets = self.programmer_targets()?;
        let mut captured = Levels::new();
        for target in &targets {
            if let Some(fixture) = self.show.fixture(*target) {
                for (attribute, level) in &fixture.levels {
                    captured.set(*target, attribute, *level);
                }
            }
        }

        let cue_name = name.clone().unwrap_or_default();
        let name_label = name
            .as_ref()
            .map(|name| format!(" \"{name}\""))
            .unwrap_or_default();
        let mut created = false;
        let mut values = 0usize;

        let message = self.mutate(|show| {
            let index = match show.sequence_index(sequence_number) {
                Some(index) => index,
                None => {
                    show.sequences.push(Sequence::new(sequence_number));
                    show.sequences.len() - 1
                }
            };
            let sequence = &mut show.sequences[index];
            let existing = sequence.cue_index(number);
            let insertion = existing.unwrap_or_else(|| sequence.insertion_index(number));
            let previous = sequence.state_before(insertion);
            let delta = captured.difference(&previous);
            values = delta.len();
            let fade = fade.unwrap_or_else(|| {
                existing
                    .and_then(|index| sequence.cues.get(index))
                    .map(|cue| cue.fade)
                    .unwrap_or(0.0)
            });
            let delay = delay.unwrap_or_else(|| {
                existing
                    .and_then(|index| sequence.cues.get(index))
                    .map(|cue| cue.delay)
                    .unwrap_or(0.0)
            });
            created = existing.is_none();
            match existing {
                Some(index) => {
                    let cue = &mut sequence.cues[index];
                    if name.is_some() {
                        cue.name = cue_name.clone();
                    }
                    cue.fade = fade;
                    cue.delay = delay;
                    cue.levels = delta;
                }
                None => sequence.cues.insert(
                    insertion,
                    crate::playback::Cue {
                        number,
                        name: cue_name.clone(),
                        fade,
                        delay,
                        levels: delta,
                    },
                ),
            }
            show.sequences.sort_by_key(|sequence| sequence.number);
            Ok(format!(
                "{} cue {}{name_label} in sequence {sequence_number} (fade {}s, delay {}s, {values} tracked values)",
                if created { "Stored" } else { "Updated" },
                format_cue_number(number),
                fade,
                delay
            ))
        })?;

        let mut lines = vec![message];
        if values == 0 {
            lines.push(
                "The cue stores no values: the programmer is empty or already matches the previous cue."
                    .to_string(),
            );
        }
        Ok(CommandOutcome::with_lines(lines))
    }

    fn delete_cue(
        &mut self,
        sequence_number: u16,
        number: f64,
    ) -> Result<CommandOutcome, CommandError> {
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "`Sequence` lists the sequences",
            ));
        };
        if sequence.cue_index(number).is_none() {
            return Err(CommandError::with_hint(
                format!(
                    "sequence {sequence_number} has no cue {}",
                    format_cue_number(number)
                ),
                "`Sequence {sequence_number}` lists its cues",
            ));
        }
        let message = self.mutate(|show| {
            if let Some(index) = show.sequence_index(sequence_number) {
                let cues = &mut show.sequences[index].cues;
                if let Some(position) = cues
                    .iter()
                    .position(|cue| (cue.number - number).abs() < 1e-9)
                {
                    cues.remove(position);
                }
            }
            Ok(format!(
                "Deleted cue {} from sequence {sequence_number}",
                format_cue_number(number)
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn sequence_list(&self, only: Option<u16>) -> CommandOutcome {
        match only {
            Some(number) => match self.show.sequence(number) {
                None => CommandOutcome::message(format!("Sequence {number} does not exist.")),
                Some(sequence) => {
                    let mut lines = vec![format!(
                        "Sequence {} \"{}\"",
                        sequence.number, sequence.name
                    )];
                    if sequence.cues.is_empty() {
                        lines.push("No cues stored.".to_string());
                        return CommandOutcome::with_lines(lines);
                    }
                    lines.push(format!(
                        "{:<6} {:<20} {:>6} {:>6} {}",
                        "Cue", "Name", "Fade", "Delay", "Values"
                    ));
                    for cue in &sequence.cues {
                        lines.push(format!(
                            "{:<6} {:<20} {:>5.1}s {:>5.1}s {}{}",
                            format_cue_number(cue.number),
                            truncate(&cue.name, 20),
                            cue.fade,
                            cue.delay,
                            cue.levels.len(),
                            if cue.levels.is_empty() {
                                " (empty)"
                            } else {
                                ""
                            }
                        ));
                    }
                    CommandOutcome::with_lines(lines)
                }
            },
            None => {
                if self.show.sequences.is_empty() {
                    return CommandOutcome::message(
                        "No sequences. Store a cue first: `Store Cue 1`.",
                    );
                }
                let mut lines = vec![format!(
                    "{:<4} {:<20} {:<6} {}",
                    "No.", "Name", "Cues", "Last cue"
                )];
                for sequence in &self.show.sequences {
                    let last = sequence
                        .cues
                        .last()
                        .map(|cue| format_cue_number(cue.number))
                        .unwrap_or_else(|| "-".to_string());
                    lines.push(format!(
                        "{:<4} {:<20} {:<6} {}",
                        sequence.number,
                        truncate(&sequence.name, 20),
                        sequence.cues.len(),
                        last
                    ));
                }
                CommandOutcome::with_lines(lines)
            }
        }
    }

    fn assign_executor(
        &mut self,
        number: u16,
        sequence: u16,
    ) -> Result<CommandOutcome, CommandError> {
        if self.show.sequence(sequence).is_none() {
            return Err(CommandError::with_hint(
                format!("sequence {sequence} does not exist"),
                "store a cue first: `Store Cue 1`",
            ));
        }
        let existed = self.show.executor(number).is_some();
        let message = self.mutate(|show| {
            match show
                .executors
                .iter_mut()
                .find(|executor| executor.number == number)
            {
                Some(executor) => executor.sequence = sequence,
                None => show.executors.push(Executor {
                    number,
                    sequence,
                    name: format!("Executor {number}"),
                    fader: 1.0,
                }),
            }
            show.executors.sort_by_key(|executor| executor.number);
            Ok(format!(
                "{} executor {number} to sequence {sequence}",
                if existed { "Reassigned" } else { "Assigned" }
            ))
        })?;
        // A reassigned executor must not keep running its old sequence.
        if self.playback.release(Target::Executor(number)) {
            self.refresh_playback();
        }
        Ok(CommandOutcome::message(message))
    }

    fn set_executor_level(
        &mut self,
        number: u16,
        level: Level,
    ) -> Result<CommandOutcome, CommandError> {
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
        if self.show.executor(number).is_none() {
            return Err(CommandError::with_hint(
                format!("executor {number} does not exist"),
                "assign one first: `Assign Executor 1 Sequence 1`",
            ));
        }
        let percent = (value * 100.0).round() as i64;
        let message = self.mutate(|show| {
            if let Some(executor) = show
                .executors
                .iter_mut()
                .find(|executor| executor.number == number)
            {
                executor.fader = value;
            }
            Ok(format!("Executor {number} fader at {percent}%"))
        })?;
        self.refresh_playback();
        Ok(CommandOutcome::message(message))
    }

    fn executor_list(&self, only: Option<u16>) -> CommandOutcome {
        let executors: Vec<&Executor> = match only {
            Some(number) => match self.show.executor(number) {
                Some(executor) => vec![executor],
                None => {
                    return CommandOutcome::message(format!("Executor {number} does not exist."))
                }
            },
            None => self.show.executors.iter().collect(),
        };
        if executors.is_empty() {
            return CommandOutcome::message(
                "No executors. Assign one: `Assign Executor 1 Sequence 1`.",
            );
        }

        let mut lines = vec![format!(
            "{:<4} {:<20} {:<8} {:<9} {}",
            "No.", "Sequence", "Fader", "State", "Cue"
        )];
        for executor in executors {
            let (state, cue) = match self.playback.run(Target::Executor(executor.number)) {
                Some(run) => {
                    let state = if run.is_paused() {
                        "paused".to_string()
                    } else if run.done {
                        "complete".to_string()
                    } else {
                        format!("{:.0}%", run.progress(self.clock) * 100.0)
                    };
                    let cue = self
                        .show
                        .sequence(run.sequence)
                        .and_then(|sequence| sequence.cues.get(run.cue_index))
                        .map(|cue| format_cue_number(cue.number))
                        .unwrap_or_else(|| "-".to_string());
                    (state, cue)
                }
                None => ("idle".to_string(), "-".to_string()),
            };
            lines.push(format!(
                "{:<4} {:<20} {:<8} {:<9} {}",
                executor.number,
                format!("Sequence {}", executor.sequence),
                format!("{:.0}%", executor.fader * 100.0),
                state,
                cue
            ));
        }
        CommandOutcome::with_lines(lines)
    }

    fn resolve_target(&self, reference: Option<PlaybackRef>) -> Result<Target, CommandError> {
        match reference {
            Some(PlaybackRef::Executor(number)) => {
                if self.show.executor(number).is_none() {
                    return Err(CommandError::with_hint(
                        format!("executor {number} does not exist"),
                        "assign one: `Assign Executor 1 Sequence 1`",
                    ));
                }
                Ok(Target::Executor(number))
            }
            Some(PlaybackRef::Sequence(number)) => {
                if self.show.sequence(number).is_none() {
                    return Err(CommandError::with_hint(
                        format!("sequence {number} does not exist"),
                        "store a cue first: `Store Cue 1`",
                    ));
                }
                Ok(Target::Sequence(number))
            }
            None => {
                if let Some(executor) = self.show.executors.first() {
                    Ok(Target::Executor(executor.number))
                } else if let Some(sequence) = self.show.sequences.first() {
                    Ok(Target::Sequence(sequence.number))
                } else {
                    Err(CommandError::with_hint(
                        "no playback to run",
                        "store a cue and assign an executor: `Store Cue 1`, then `Assign Executor 1 Sequence 1`",
                    ))
                }
            }
        }
    }

    fn sequence_number_for(&self, target: Target) -> Result<u16, CommandError> {
        match target {
            Target::Sequence(number) => Ok(number),
            Target::Executor(number) => self
                .show
                .executor(number)
                .map(|executor| executor.sequence)
                .ok_or_else(|| CommandError::new(format!("executor {number} does not exist"))),
        }
    }

    fn refresh_playback(&mut self) {
        self.playback.refresh(&self.show, self.clock);
        self.revision += 1;
    }

    fn running_cue_label(&self, target: Target) -> String {
        let Some(run) = self.playback.run(target) else {
            return "cue -".to_string();
        };
        self.show
            .sequence(run.sequence)
            .and_then(|sequence| sequence.cues.get(run.cue_index))
            .map(|cue| format!("cue {}", format_cue_number(cue.number)))
            .unwrap_or_else(|| "cue -".to_string())
    }

    fn playback_go(
        &mut self,
        reference: Option<PlaybackRef>,
    ) -> Result<CommandOutcome, CommandError> {
        let target = self.resolve_target(reference)?;
        let sequence_number = self.sequence_number_for(target)?;

        if self
            .playback
            .run(target)
            .map(|run| run.is_paused())
            .unwrap_or(false)
        {
            self.playback.resume(target, self.clock);
            self.refresh_playback();
            return Ok(CommandOutcome::message(format!(
                "Resumed {} at {}",
                target.label(),
                self.running_cue_label(target)
            )));
        }

        let Some(sequence) = self.show.sequence(sequence_number).cloned() else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "store a cue first: `Store Cue 1`",
            ));
        };
        if sequence.cues.is_empty() {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} has no cues"),
                "store one first: `Store Cue 1`",
            ));
        }

        let index = match self.playback.run(target) {
            Some(run) => run.cue_index + 1,
            None => 0,
        };
        if index >= sequence.cues.len() {
            let last = sequence
                .cues
                .last()
                .map(|cue| format_cue_number(cue.number))
                .unwrap_or_default();
            return Ok(CommandOutcome::message(format!(
                "{} is at the last cue ({last})",
                target.label()
            )));
        }

        self.playback
            .go(&self.show, target, sequence_number, index, self.clock)
            .map_err(CommandError::new)?;
        self.refresh_playback();

        let cue = &sequence.cues[index];
        let name = if cue.name.is_empty() {
            String::new()
        } else {
            format!(" \"{}\"", cue.name)
        };
        Ok(CommandOutcome::message(format!(
            "{} → cue {}{name} (fade {}s, delay {}s)",
            target.label(),
            format_cue_number(cue.number),
            cue.fade,
            cue.delay
        )))
    }

    fn playback_back(
        &mut self,
        reference: Option<PlaybackRef>,
    ) -> Result<CommandOutcome, CommandError> {
        let target = self.resolve_target(reference)?;
        let sequence_number = self.sequence_number_for(target)?;
        let Some(cue_index) = self.playback.run(target).map(|run| run.cue_index) else {
            return Ok(CommandOutcome::message(format!(
                "{} is not running",
                target.label()
            )));
        };
        if cue_index == 0 {
            return Ok(CommandOutcome::message(format!(
                "{} is at the first cue",
                target.label()
            )));
        }
        let index = cue_index - 1;
        self.playback
            .go(&self.show, target, sequence_number, index, self.clock)
            .map_err(CommandError::new)?;
        self.refresh_playback();
        Ok(CommandOutcome::message(format!(
            "{} ← {}",
            target.label(),
            self.running_cue_label(target)
        )))
    }

    fn playback_pause(
        &mut self,
        reference: Option<PlaybackRef>,
    ) -> Result<CommandOutcome, CommandError> {
        let target = self.resolve_target(reference)?;
        if self.playback.pause(target, self.clock) {
            self.refresh_playback();
            Ok(CommandOutcome::message(format!(
                "Paused {} at {}",
                target.label(),
                self.running_cue_label(target)
            )))
        } else {
            Ok(CommandOutcome::message(format!(
                "{} is not fading",
                target.label()
            )))
        }
    }

    fn playback_release(
        &mut self,
        reference: Option<PlaybackRef>,
    ) -> Result<CommandOutcome, CommandError> {
        let target = self.resolve_target(reference)?;
        if self.playback.release(target) {
            self.refresh_playback();
            Ok(CommandOutcome::message(format!(
                "Released {}; output returns to the programmer",
                target.label()
            )))
        } else {
            Ok(CommandOutcome::message(format!(
                "{} is not running",
                target.label()
            )))
        }
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
    "Fixture 1 At ColorAdd_R 40             set any attribute of the mode",
    "At Palette Color 3                     apply a stored palette",
    "At 50 / Full / Off                     apply to the current selection",
    "Clear | Clear Values | Clear All        selection / programmer values",
    "Patch Fixture 1 At 1.001 Type \"<fixture type>\" [Mode \"<mode>\"]",
    "Unpatch Fixture 1 Thru 3",
    "Import Gdtf \"file.gdtf\"                load a GDTF fixture type",
    "New [Show] [\"name\"]                    start an empty show",
    "Store Palette Color 1 \"Warm\"           capture the programmer in a pool",
    "Delete Palette Color 1 / Palette        delete or list palettes",
    "Store Cue 1 [\"name\"] [Fade 2] [Delay 0.5] [Sequence 2]",
    "Delete Cue 1 [Sequence 2] / Sequence [2]  delete or list cues",
    "Assign Executor 1 Sequence 1            bind a cue list to a fader",
    "Executor 1 At 75 | Full | Off           executor master fader",
    "Go | Back | Pause | Release [Executor 1 | Sequence 1]",
    "Sheet                                  print the fixture sheet",
    "Status                                 output, show and playback state",
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

fn attribute_list(mode: &crate::fixture_type::Mode) -> String {
    let mut attributes: Vec<String> = mode
        .channels
        .iter()
        .map(|channel| channel.attribute.clone())
        .collect();
    attributes.sort();
    attributes.dedup();
    attributes.join(", ")
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
    use std::collections::BTreeMap;
    use std::time::Duration;

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
        assert!(
            engine.show.fixture(1).unwrap().levels.is_empty(),
            "a rejected command changed nothing"
        );
        assert_eq!(engine.render().slot(1, 1), Some(0));
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
        assert!(
            fixture.levels.is_empty(),
            "a fresh fixture has an empty programmer, so playback can drive it"
        );
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

    #[test]
    fn store_palette_captures_and_applies_per_type() {
        let mut engine = engine();
        engine.execute("Fixture 1 At ColorAdd_R Full").unwrap();
        let outcome = engine.execute("Store Palette Color 1 \"Red\"").unwrap();
        assert!(
            outcome.lines[0].contains("Stored palette Color 1"),
            "{outcome:?}"
        );
        let palette = engine.show.palette("color", 1).unwrap();
        assert_eq!(palette.name, "Red");
        assert_eq!(palette.values["OpenLight Demo Wash"]["ColorAdd_R"], 1.0);
        assert_eq!(
            palette.values["OpenLight Demo Wash"]["ColorAdd_B"], 0.0,
            "a colour palette stores the whole colour system"
        );

        engine
            .execute(r#"Patch Fixture 2 At 1.010 Type "OpenLight Demo Wash""#)
            .unwrap();
        engine.execute("Fixture 2 At Palette Color 1").unwrap();
        let fixture = engine.show.fixture(2).unwrap();
        assert_eq!(fixture.levels.get("ColorAdd_R"), Some(&1.0));
        assert_eq!(fixture.levels.get("ColorAdd_B"), Some(&0.0));
        assert_eq!(engine.render().slot(1, 11), Some(255));
    }

    #[test]
    fn palette_commands_report_missing_objects() {
        let mut engine = engine();
        let error = engine.execute("At Palette Color 9").unwrap_err();
        assert_eq!(error.message, "nothing selected");

        engine.execute("Fixture 1").unwrap();
        let error = engine.execute("At Palette Color 9").unwrap_err();
        assert!(error.message.contains("does not exist"), "{error}");
        assert!(error.hint.unwrap().contains("Store Palette"));

        let error = engine.execute("Store Palette Position 1").unwrap_err();
        assert!(
            error.message.contains("nothing in the selection"),
            "{error}"
        );

        let error = engine.execute("Fixture 1 At Nonsense 50").unwrap_err();
        assert!(error.message.contains("no attribute"), "{error}");
        assert!(error.hint.unwrap().contains("Dimmer"));
    }

    #[test]
    fn cues_track_and_play_back_with_fades() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);

        engine.execute("Fixture 1 At 25").unwrap();
        engine.execute("Store Cue 1 Fade 1").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 2 Fade 1").unwrap();

        let sequence = engine.show.sequence(1).unwrap();
        assert_eq!(sequence.cues.len(), 2);
        assert_eq!(
            sequence.cues[1].levels.len(),
            1,
            "cue 2 stores only the tracked change"
        );

        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start + Duration::from_secs(1));
        assert_eq!(engine.render().slot(1, 1), Some(64), "cue 1 at 25%");

        engine.execute("Go Sequence 1").unwrap();
        let half = engine.clock() + Duration::from_millis(500);
        engine.tick(half);
        assert_eq!(engine.render().slot(1, 1), Some(159), "half way to 100%");

        engine.tick(start + Duration::from_secs(3));
        assert_eq!(engine.render().slot(1, 1), Some(255));
    }

    #[test]
    fn executor_fader_scales_a_running_cue() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Assign Executor 1 Sequence 1").unwrap();
        engine.execute("Executor 1 At 50").unwrap();
        engine.execute("Go").unwrap();
        engine.tick(start);
        assert_eq!(engine.render().slot(1, 1), Some(128));

        engine.execute("Executor 1 Full").unwrap();
        engine.tick(start);
        assert_eq!(engine.render().slot(1, 1), Some(255));

        engine.execute("Executor 1 Off").unwrap();
        engine.tick(start);
        assert_eq!(engine.render().slot(1, 1), Some(0));
    }

    #[test]
    fn back_pause_and_release_drive_the_same_run() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 2 Fade 2").unwrap();
        engine.execute("Clear All").unwrap();

        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start);
        assert_eq!(engine.render().slot(1, 1), Some(51));

        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start + Duration::from_millis(500));
        let fading = engine.render().slot(1, 1).unwrap();
        assert!(fading > 51 && fading < 255, "mid fade, got {fading}");

        engine.execute("Pause Sequence 1").unwrap();
        let paused = engine.render().slot(1, 1);
        engine.tick(start + Duration::from_secs(1));
        assert_eq!(engine.render().slot(1, 1), paused, "pause freezes output");

        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start + Duration::from_secs(5));
        assert_eq!(engine.render().slot(1, 1), Some(255), "resume finishes");

        let outcome = engine.execute("Back Sequence 1").unwrap();
        assert!(outcome.lines[0].contains("cue 1"), "{outcome:?}");
        engine.tick(start + Duration::from_secs(6));
        assert_eq!(engine.render().slot(1, 1), Some(51));

        assert!(engine.execute("Release Sequence 1").unwrap().lines[0].contains("Released"));
        engine.tick(start + Duration::from_secs(7));
        assert_eq!(engine.render().slot(1, 1), Some(0));
    }

    #[test]
    fn clear_values_hands_a_fixture_back_to_playback() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear Values").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start);
        assert_eq!(engine.render().slot(1, 1), Some(102));

        engine.execute("Fixture 1 At Full").unwrap();
        engine.tick(start);
        assert_eq!(
            engine.render().slot(1, 1),
            Some(255),
            "the programmer wins while it holds a value"
        );
        engine.execute("Clear Values").unwrap();
        engine.tick(start);
        assert_eq!(engine.render().slot(1, 1), Some(102));
    }

    #[test]
    fn store_cue_uses_the_previous_cue_as_its_baseline() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 30").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Store Cue 1.5 Fade 0.5").unwrap();
        let sequence = engine.show.sequence(1).unwrap();
        assert_eq!(sequence.cues.len(), 2);
        assert_eq!(
            sequence.cues[1].levels.len(),
            0,
            "nothing changed since cue 1"
        );
        assert_eq!(sequence.cues[1].fade, 0.5);

        engine.execute("Fixture 1 At 60").unwrap();
        engine.execute("Store Cue 2 \"Brighter\"").unwrap();
        let sequence = engine.show.sequence(1).unwrap();
        assert_eq!(sequence.cues.len(), 3);
        assert_eq!(sequence.cues[2].name, "Brighter");
        assert_eq!(
            sequence.cue(2.0).unwrap().levels.get(1, "Dimmer"),
            Some(0.6)
        );
        assert_eq!(sequence.cue(2.0).unwrap().levels.len(), 1);
    }

    #[test]
    fn playback_commands_report_bad_targets_and_ends() {
        let mut engine = engine();
        let error = engine.execute("Go").unwrap_err();
        assert!(error.message.contains("no playback"), "{error}");

        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        let outcome = engine.execute("Go Sequence 1").unwrap();
        assert!(outcome.lines[0].contains("last cue"), "{outcome:?}");

        let error = engine.execute("Assign Executor 1 Sequence 9").unwrap_err();
        assert!(error.message.contains("sequence 9"), "{error}");
        let error = engine.execute("Executor 2 At 50").unwrap_err();
        assert!(error.message.contains("executor 2"), "{error}");
        assert!(engine.execute("Delete Cue 7 Sequence 1").is_err());
    }

    #[test]
    fn loading_a_show_stops_the_previous_playback() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        assert!(engine.playback.is_running());

        let path = std::env::temp_dir().join(format!(
            "openlight-phase2-{}-load.olshow.json",
            std::process::id()
        ));
        engine
            .execute(&format!("Save \"{}\"", path.display()))
            .unwrap();
        engine
            .execute(&format!("Load \"{}\"", path.display()))
            .unwrap();
        assert!(!engine.playback.is_running());
        assert_eq!(engine.render().slot(1, 1), Some(0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn new_show_resets_the_programmer_and_playback() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Assign Executor 1 Sequence 1").unwrap();
        let outcome = engine.execute("New Show \"Gig\"").unwrap();
        assert!(outcome.lines[0].contains("Gig"));
        assert!(engine.show.fixtures.is_empty());
        assert!(engine.show.sequences.is_empty());
        assert!(!engine.playback.is_running());
        assert!(engine.selection.is_empty());
    }

    #[test]
    fn palettes_cues_and_executors_survive_save_and_load() {
        let mut engine = engine();
        engine.execute("Fixture 1 At ColorAdd_R Full").unwrap();
        engine.execute("Store Palette Color 1 \"Red\"").unwrap();
        engine.execute("Store Cue 1 Fade 1.5").unwrap();
        engine.execute("Assign Executor 1 Sequence 1").unwrap();
        let path = std::env::temp_dir().join(format!(
            "openlight-phase2-{}.olshow.json",
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
        assert_eq!(other.show.palettes.len(), 1);
        assert_eq!(other.show.sequences[0].cues[0].fade, 1.5);
        assert_eq!(other.show.executors[0].sequence, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn listings_show_palettes_sequences_and_executors() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Palette Dimmer 1 \"Low\"").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Assign Executor 1 Sequence 1").unwrap();

        let palettes = engine.execute("Palette").unwrap().lines.join("\n");
        assert!(
            palettes.contains("Dimmer") && palettes.contains("Low"),
            "{palettes}"
        );
        let sequences = engine.execute("Sequence").unwrap().lines.join("\n");
        assert!(sequences.contains("Main") || sequences.contains("Sequence 1"));
        let cues = engine.execute("Sequence 1").unwrap().lines.join("\n");
        assert!(cues.contains("Cue"), "{cues}");
        let executors = engine.execute("Executor").unwrap().lines.join("\n");
        assert!(executors.contains("idle"), "{executors}");

        let status = engine.execute("Status").unwrap().lines.join("\n");
        assert!(status.contains("Palettes: 1"), "{status}");
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

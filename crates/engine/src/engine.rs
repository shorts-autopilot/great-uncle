//! Engine façade.
//!
//! `Engine` owns the show, the transient selection and the output status. Every
//! command goes through [`Engine::execute`]; mutations run inside
//! [`Engine::mutate`], which rolls back and reports if the result would not
//! validate, so a show is always safe to render and save.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::command::{
    self, Command, CommandError, Level, LevelSource, PlaybackRef, ReportKind, SetTarget, StoreMode,
    TriggerArg,
};
use crate::csv;
use crate::desk::{Desk, Masters, Preview, RunState};
use crate::effect::{Effect, Effects, Waveform, MAX_RATE_HZ, MAX_SPREAD_DEGREES};
use crate::fixture_type::{Channel, FixtureType, SetSelector, DIMMER};
use crate::gdtf;
use crate::history::{History, Snapshot};
use crate::journal::ReplayFrom;
use crate::levels::Levels;
use crate::output::{lock, OutputStatus};
use crate::palette::{self, Palette};
use crate::patch::{self, PatchRange};
use crate::playback::{
    format_cue_number, format_trigger, timing_for, Cue, Curve, Executor, ExecutorMode, Playback,
    Sequence, Target, Trigger, MAX_CUE_NUMBER, MAX_TIME,
};
use crate::pools::{Filter, Group, World};
use crate::render::{self, Highlight, OutputFrame, Stage};
use crate::show::{Fixture, Show, MAX_FIXTURES, MAX_RATE_MASTER};
use crate::timecode::{self, TimecodeEvent, TimecodePlayer, TimecodeShow, MAX_FPS, MIN_FPS};

/// Everything `Store Cue` carries before defaults are applied.
struct CueSpec {
    sequence: u16,
    number: f64,
    name: Option<String>,
    fade: Option<f64>,
    delay: Option<f64>,
    mib: Option<bool>,
    trigger: Option<TriggerArg>,
    mode: StoreMode,
}

/// Everything `Cue <n> Timing …` carries before it is applied.
struct TimingSpec {
    attribute: Option<String>,
    fade: Option<f64>,
    delay: Option<f64>,
    curve: Option<Curve>,
    off: bool,
}

/// Everything `Store Effect` carries before defaults are applied.
struct EffectSpec {
    number: u16,
    name: Option<String>,
    attribute: Option<String>,
    waveform: Option<String>,
    rate: Option<f64>,
    size: Option<f64>,
    spread: Option<f64>,
}

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

/// What a journal replay achieved.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Statements that replayed cleanly.
    pub applied: usize,
    /// Statements skipped because they no longer apply, with the reason.
    pub skipped: Vec<String>,
    /// The revision that broke the chain, if any.
    pub stopped: Option<u64>,
    /// The oldest revision still in the journal when its head was lost to
    /// rotation, so earlier statements could not be replayed.
    pub head_lost: Option<u64>,
    /// A final journal line cut short by the crash.
    pub incomplete: usize,
}

impl RecoveryReport {
    pub fn summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.applied > 0 {
            lines.push(format!("Recovered {} edits from the journal", self.applied));
        }
        for skipped in &self.skipped {
            lines.push(format!("Warning: skipped {skipped}"));
        }
        if let Some(rev) = self.stopped {
            lines.push(format!(
                "Warning: journal replay stopped at revision {rev}; later entries ignored"
            ));
        }
        if let Some(rev) = self.head_lost {
            lines.push(format!(
                "Warning: the journal begins at revision {rev}; earlier edits are not available"
            ));
        }
        if self.incomplete > 0 {
            lines.push("Warning: 1 incomplete journal entry was dropped".to_string());
        }
        lines
    }

    pub fn recovered_anything(&self) -> bool {
        self.applied > 0 || !self.skipped.is_empty() || self.incomplete > 0
    }
}

pub struct Engine {
    pub show: Show,
    /// The handles: selection, programmer, masters, playback positions.
    pub desk: Desk,
    /// Bumped on every change the output threads must pick up.
    pub revision: u64,
    pub status: Arc<Mutex<OutputStatus>>,
    /// Running cues. Session state; the desk records where they are.
    pub playback: Playback,
    /// Running effects. Not part of the show file.
    pub effects: Effects,
    /// Timecode playhead. Not part of the show file.
    pub timecode: TimecodePlayer,
    /// Show snapshots for `Undo` and `Redo`.
    history: History,
    /// The statement being dispatched, recorded as its undo label.
    label: String,
    /// The crash journal, when one is open.
    journal: Option<crate::journal::Journal>,
    /// The head of the journal file when this session does not append to it.
    /// Snapshots record it, so a later replay starts after the entries they
    /// already contain even though no journal was open here.
    journal_head: u64,
    /// Autosave target, interval and the last time it ran.
    autosave: Option<(PathBuf, Duration, Instant)>,
    /// Why the last autosave failed, reported once in `Status`.
    autosave_error: Option<String>,
    /// True while replaying the journal: no journalling, no undo steps.
    recovering: bool,
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
            desk: Desk::new(),
            revision: 0,
            status: Arc::new(Mutex::new(OutputStatus::default())),
            playback: Playback::new(),
            effects: Effects::default(),
            timecode: TimecodePlayer::default(),
            history: History::default(),
            label: String::new(),
            journal: None,
            journal_head: 0,
            autosave: None,
            autosave_error: None,
            recovering: false,
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
        // Timecode events start cues before the playback is sampled, so a cue
        // that begins on this frame is already in the output.
        let fired = self.timecode_events(now);
        let mut started = BTreeSet::new();
        for event in &fired {
            started.insert(Target::Sequence(event.sequence));
            self.fire_timecode_event(event, now);
        }
        // A `Follow` or `Time` trigger that came due starts through the same
        // path, so a chained cue is already in this frame. A target timecode
        // started on this tick is left alone: its own chain begins next tick.
        let due = self.playback.due(&self.show, now);
        let chained = !due.is_empty();
        for (target, sequence, index) in due {
            if started.contains(&target) {
                continue;
            }
            self.start_cue(target, sequence, index, now);
        }
        let advanced = self.playback.refresh(&self.desk, now);
        // A running effect or timecode changes the output on every tick, even
        // when no cue is moving, so the sender keeps re-rendering.
        let changed = advanced
            || !fired.is_empty()
            || chained
            || self.effects.is_running()
            || self.timecode.is_running();
        if changed {
            self.revision += 1;
        }
        if let Some((path, interval, last)) = &mut self.autosave {
            if now.duration_since(*last) >= *interval {
                *last = now;
                let path = path.clone();
                if let Err(error) = self.save_snapshot(&path) {
                    self.autosave_error = Some(error.to_string());
                } else {
                    self.autosave_error = None;
                }
            }
        }
        changed
    }

    /// Start (or stop, with a zero interval) periodic autosave.
    pub fn enable_autosave(&mut self, path: PathBuf, interval: Duration) {
        self.autosave = if interval.is_zero() {
            None
        } else {
            Some((path, interval, Instant::now()))
        };
    }

    pub fn set_journal(&mut self, journal: Option<crate::journal::Journal>) {
        self.journal = journal;
    }

    /// Tell the engine where the journal file stands when no journal is open.
    /// The console reads the file once at startup, so a snapshot taken with
    /// `--journal off` still records a real revision.
    pub fn set_journal_head(&mut self, rev: u64) {
        self.journal_head = rev;
    }

    pub fn journal(&self) -> Option<&crate::journal::Journal> {
        self.journal.as_ref()
    }

    /// Write a snapshot of the document and the desk right now.
    pub fn autosave_now(&mut self) -> io::Result<()> {
        let Some((path, _, _)) = &self.autosave else {
            return Ok(());
        };
        let path = path.clone();
        self.save_snapshot(&path)
    }

    fn save_snapshot(&self, path: &Path) -> io::Result<()> {
        // The revision a snapshot records is a journal revision, because that
        // is what replay compares it against. Engine revisions count renders
        // and restart at zero, so using one here would replay statements the
        // snapshot already contains. With no journal open the console still
        // tells us the file's head, which is the honest claim: older entries
        // are already in this document, newer ones are not.
        let rev = self
            .journal
            .as_ref()
            .map(crate::journal::Journal::rev)
            .unwrap_or(self.journal_head);
        crate::journal::Snapshot::capture(Some(rev), &self.show, &self.desk).save(path)
    }

    /// Install a recovered document and desk, restoring playback positions.
    pub fn adopt(&mut self, show: Show, desk: Desk) {
        self.show = show;
        self.desk = desk;
        self.playback = Playback::new();
        self.effects.stop_all();
        self.timecode.stop();
        self.history = History::default();
        self.restore_desk_runs();
        self.revision += 1;
    }

    /// Replay the journal tail from `from`, the revision the document is at.
    ///
    /// Replay is ordinary [`Engine::execute`] with journalling and undo
    /// recording off, so recovery cannot disagree with the command language. A
    /// statement that no longer applies (a fixture deleted later) is skipped
    /// with a warning; a break in the revision chain stops replay, because a
    /// spliced file must not silently reorder edits.
    pub fn recover(
        &mut self,
        tail: &crate::journal::Tail,
        show_path: &Path,
        from: ReplayFrom,
    ) -> RecoveryReport {
        // A `Save` records where the file on disk stands, and the autosave
        // records where a snapshot stands; replay starts after the newer one.
        // Replaying statements the document already contains would apply
        // non-idempotent edits (an `Undo`, a store that captures a programmer)
        // a second time and diverge from the saved file.
        let baseline = tail.baseline(show_path, from);
        let from = baseline.rev;
        let mut report = RecoveryReport {
            head_lost: baseline.head_lost,
            incomplete: tail.incomplete,
            ..RecoveryReport::default()
        };
        let mut expected = from;
        self.recovering = true;
        for entry in &tail.entries {
            if entry.rev <= from {
                continue;
            }
            if entry.rev != expected + 1 {
                report.stopped = Some(entry.rev);
                break;
            }
            expected = entry.rev;
            let line = entry.line.clone();
            match self.execute(&line) {
                Ok(_) => report.applied += 1,
                Err(error) => report.skipped.push(format!("`{line}`: {error}")),
            }
        }
        self.recovering = false;
        report
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
            self.label = statement.trim().to_string();
            let parsed = command::parse(&tokens)?;
            if self.desk.lock.is_some() && parsed.mutates() {
                return Err(CommandError::with_hint(
                    "the desk is locked",
                    "`Unlock <code>` releases it",
                ));
            }
            outcome.lines.extend(self.dispatch(parsed)?.lines);
        }
        Ok(outcome)
    }

    pub fn render(&self) -> OutputFrame {
        let effects = self
            .effects
            .output(&self.show, self.desk.masters.rate, self.clock);
        // Blind replaces the programmer with an empty level map for this one
        // render. Playback still runs: the programmer is the only layer that
        // goes dark.
        let empty = Levels::new();
        let programmer = if self.desk.blind {
            &empty
        } else {
            &self.desk.programmer
        };
        // A blind desk and a full blackout both outrank the highlight: an
        // output aid must not defeat either one.
        let highlight =
            if self.desk.highlight && !self.desk.blind && !self.desk.is_blacked_out(self.clock) {
                Highlight {
                    on: true,
                    fixtures: &self.desk.selection,
                }
            } else {
                Highlight::default()
            };
        render::render_with_highlight(
            &self.show,
            programmer,
            self.playback.output(),
            &effects,
            &self.desk.masters,
            self.desk.blackout_master(self.clock),
            highlight,
        )
    }

    /// Render the previewed cue with no programmer, effects or masters. Only
    /// `Sheet` reads this value; it never reaches `Shared` or a transport.
    pub fn preview_frame(&self) -> Option<OutputFrame> {
        let preview = self.desk.preview?;
        let sequence = self.show.sequence(preview.sequence)?;
        let index = sequence.cue_index(preview.cue)?;
        let levels = sequence.state_at(index);
        Some(render::render(
            &self.show,
            &Levels::new(),
            &levels,
            &crate::effect::EffectOutput::default(),
            &Masters::new(),
            1.0,
        ))
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
            Command::SetChannelSet {
                fixtures,
                group,
                attribute,
                target,
            } => self.set_channel_set(fixtures, group, attribute, target),
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
            Command::Clone {
                fixture,
                universe,
                address,
                count,
                step,
            } => self.clone_fixtures(fixture, universe, address, count, step),
            Command::PatchCheck => Ok(self.patch_check()),
            Command::ImportPatch { path } => self.import_patch(&path),
            Command::ImportGdtf { path } => self.import_gdtf(&path),
            Command::New { name } => {
                let outcome = self.new_show(name)?;
                self.history.clear();
                Ok(outcome)
            }
            Command::SaveShow { path } => self.save(&path, false, true),
            Command::SaveDesk { path } => self.save(&path, true, false),
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
                mib,
                trigger,
                mode,
            } => self.store_cue(CueSpec {
                sequence,
                number,
                name,
                fade,
                delay,
                mib,
                trigger,
                mode,
            }),
            Command::Update { sequence, cue } => self.update_cue(sequence, cue),
            Command::CueBlock {
                sequence,
                number,
                on,
            } => self.cue_block(sequence, number, on),
            Command::CueShield {
                sequence,
                number,
                attribute,
                off,
            } => self.cue_shield(sequence, number, attribute, off),
            Command::CueShow { sequence, number } => self.cue_show(sequence, number),
            Command::CueMove {
                sequence,
                number,
                to,
            } => self.cue_move(sequence, number, to),
            Command::CueCopy {
                sequence,
                number,
                to,
                target,
            } => self.cue_copy(sequence, number, to, target),
            Command::CueTrigger {
                sequence,
                number,
                trigger,
            } => self.cue_trigger(sequence, number, trigger),
            Command::CueTiming {
                sequence,
                number,
                attribute,
                fade,
                delay,
                curve,
                off,
            } => self.cue_timing(
                sequence,
                number,
                TimingSpec {
                    attribute,
                    fade,
                    delay,
                    curve,
                    off,
                },
            ),
            Command::CueStagger {
                sequence,
                number,
                stagger,
            } => self.cue_stagger(sequence, number, stagger),
            Command::SetHome {
                fixtures,
                group,
                attribute,
                level,
                off,
            } => self.set_home(fixtures, group, attribute, level, off),
            Command::Home { fixtures, group } => self.home(fixtures, group),
            Command::SetExecutorMode { executor, mode } => self.set_executor_mode(executor, mode),
            Command::SetExecutorFlash { executor, on } => self.set_executor_flash(executor, on),
            Command::SetExecutorSolo { executor, on } => self.set_executor_solo(executor, on),
            Command::DeleteCue { sequence, number } => self.delete_cue(sequence, number),
            Command::ListSequences { only } => Ok(self.sequence_list(only)),
            Command::AssignExecutor { executor, sequence } => {
                self.assign_executor(executor, sequence)
            }
            Command::SetExecutorLevel { executor, level } => {
                self.set_executor_level(executor, level)
            }
            Command::ListExecutors { only } => Ok(self.executor_list(only)),
            Command::SelectGroup(number) => self.select_group(number),
            Command::SetLevelGroup {
                group,
                attribute,
                source,
            } => self.set_level_group(group, attribute, source),
            Command::OffGroup { group } => self.off_group(group),
            Command::StoreGroup { number, name } => self.store_group(number, name),
            Command::DeleteGroup { number } => self.delete_group(number),
            Command::ListGroups => Ok(self.group_list()),
            Command::StoreWorld { number, name } => self.store_world(number, name),
            Command::DeleteWorld { number } => self.delete_world(number),
            Command::ActivateWorld { number } => self.activate_world(number),
            Command::ListWorlds => Ok(self.world_list()),
            Command::StoreFilter {
                number,
                name,
                features,
            } => self.store_filter(number, name, features),
            Command::DeleteFilter { number } => self.delete_filter(number),
            Command::ActivateFilter { number } => self.activate_filter(number),
            Command::ListFilters => Ok(self.filter_list()),
            Command::SetGrandMaster { level } => self.set_grand_master(level),
            Command::SetRateMaster { level } => self.set_rate_master(level),
            Command::SetGroupMaster { group, level } => self.set_group_master(group, level),
            Command::ListMasters => Ok(self.master_list()),
            Command::StoreEffect {
                number,
                name,
                attribute,
                waveform,
                rate,
                size,
                spread,
            } => self.store_effect(EffectSpec {
                number,
                name,
                attribute,
                waveform,
                rate,
                size,
                spread,
            }),
            Command::DeleteEffect { number } => self.delete_effect(number),
            Command::ListEffects { only } => Ok(self.effect_list(only)),
            Command::EffectOn { effect, fixtures } => self.effect_on(effect, fixtures),
            Command::EffectOff { effect } => self.effect_off(effect),
            Command::StoreTimecode { number, name, fps } => self.store_timecode(number, name, fps),
            Command::DeleteTimecode { number } => self.delete_timecode(number),
            Command::AddTimecodeEvent {
                timecode,
                at,
                sequence,
                cue,
            } => self.add_timecode_event(timecode, at, sequence, cue),
            Command::RemoveTimecodeEvent { timecode, at } => {
                self.remove_timecode_event(timecode, at)
            }
            Command::StartTimecode { timecode } => self.start_timecode(timecode),
            Command::PauseTimecode { timecode } => self.pause_timecode(timecode),
            Command::StopTimecode { timecode } => self.stop_timecode(timecode),
            Command::LocateTimecode { timecode, at } => self.locate_timecode(timecode, &at),
            Command::ListTimecodes { only } => Ok(self.timecode_list(only)),
            Command::Undo => self.history_step(true),
            Command::Redo => self.history_step(false),
            Command::Go { target } => self.playback_go(target),
            Command::Back { target } => self.playback_back(target),
            Command::Pause { target } => self.playback_pause(target),
            Command::Release { target } => self.playback_release(target),
            Command::Blackout { off, fade } => self.set_blackout(off, fade),
            Command::Blind { on } => self.set_blind(on),
            Command::Preview { cue, sequence, off } => self.preview(cue, sequence, off),
            Command::Highlight { on } => self.set_highlight(on),
            Command::Lock { code } => self.lock_desk(code),
            Command::Unlock { code } => self.unlock_desk(code),
            Command::Sheet => Ok(self.sheet()),
            Command::Status => Ok(self.status_report()),
            Command::Report {
                kind,
                sequence,
                path,
            } => self.report(kind, sequence, &path),
            Command::Save { path } => self.save(&path, true, true),
            Command::Load { path, show_only } => self.load(&path, show_only),
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
        self.check_world(&fixtures)?;
        let message = format!(
            "Selected {} ({} fixture{})",
            format_fixture_list(&fixtures),
            fixtures.len(),
            if fixtures.len() == 1 { "" } else { "s" }
        );
        self.desk.selection = fixtures;
        // A selection change is normally invisible, but highlight renders it.
        if self.desk.highlight {
            self.revision += 1;
        }
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
                self.desk.selection = targets;
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
        self.check_attribute_allowed(&targets, &attribute)?;

        let mut keys: Vec<(u16, String)> = Vec::with_capacity(targets.len());
        for number in &targets {
            let fixture = self
                .show
                .fixture(*number)
                .ok_or_else(|| CommandError::new(format!("fixture {number} is not patched")))?;
            let (_, mode) = self.show.mode_for(fixture).map_err(CommandError::new)?;
            match mode.channel_for(&attribute) {
                Some(channel) => keys.push((*number, channel.attribute.clone())),
                None => {
                    return Err(CommandError::with_hint(
                        format!(
                            "fixture {number} has no `{attribute}` in mode `{}`",
                            fixture.mode
                        ),
                        format!("attributes: {}", attribute_list(mode)),
                    ))
                }
            }
        }

        let label = format_fixture_list(&targets);
        let percent = (value * 100.0).round() as i64;
        let canonical = attribute.clone();

        let message = self.edit_programmer(|programmer| {
            for (number, key) in &keys {
                programmer.set(*number, key, value);
            }
            format!("{label} {canonical} at {percent}%")
        });
        // The fixtures you just addressed stay selected, so a following `Full`
        // or `Off` applies to the same set.
        self.desk.selection = targets;
        Ok(CommandOutcome::message(message))
    }

    /// `Fixture 1 Gobo [2 | "Rotate CW"]` and the group form: list a
    /// channel's sets, or store the midpoint of one as an ordinary level so it
    /// fades and stores like every other value.
    fn set_channel_set(
        &mut self,
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
        attribute: String,
        target: Option<SetTarget>,
    ) -> Result<CommandOutcome, CommandError> {
        let targets = match group {
            Some(number) => self.group_targets(number)?,
            None => self.targets(fixtures)?,
        };
        let canonical = self.resolve_attribute(&targets, &attribute)?;

        // Copy the channels out before the programmer borrow starts; the mode
        // list is small and this keeps the two phases independent.
        let mut addressed: Vec<(u16, Channel)> = Vec::new();
        let mut missing = Vec::new();
        for number in &targets {
            let Some(fixture) = self.show.fixture(*number) else {
                continue;
            };
            let (_, mode) = self.show.mode_for(fixture).map_err(CommandError::new)?;
            match mode.channel_for(&canonical) {
                Some(channel) => addressed.push((*number, channel.clone())),
                None => missing.push(*number),
            }
        }

        let mut lines = Vec::new();
        match target {
            None => {
                let mut seen = BTreeSet::new();
                for (number, channel) in &addressed {
                    let Some(fixture) = self.show.fixture(*number) else {
                        continue;
                    };
                    if !seen.insert((
                        fixture.type_key.clone(),
                        fixture.mode.clone(),
                        canonical.clone(),
                    )) {
                        continue;
                    }
                    lines.push(format!(
                        "Fixture {number} {canonical} (16-bit: {})",
                        if channel.fine_offset.is_some() {
                            "yes"
                        } else {
                            "no"
                        }
                    ));
                    let (min, max) = channel.physical_range().unwrap_or((0.0, 0.0));
                    let percent = |value: f64| {
                        if max > min {
                            ((value - min) / (max - min)).clamp(0.0, 1.0) * 100.0
                        } else {
                            0.0
                        }
                    };
                    for (index, function) in channel.sets() {
                        lines.push(format!(
                            "  {index:<2} {:<12} {:>3}..{}%   (DMX {}..{})",
                            truncate(&function.name, 12),
                            percent(function.physical_from.min(function.physical_to)).round()
                                as i64,
                            percent(function.physical_from.max(function.physical_to)).round()
                                as i64,
                            function.dmx_from.min(function.dmx_to),
                            function.dmx_from.max(function.dmx_to)
                        ));
                    }
                    let level = self
                        .desk
                        .programmer
                        .get(*number, &canonical)
                        .or_else(|| self.playback.output().get(*number, &canonical))
                        .unwrap_or_else(|| self.show.home_level(fixture, &canonical));
                    let raw = channel.raw16_from_level(level);
                    let coarse = (raw >> 8) as u8;
                    let current = channel.sets().find(|(_, function)| {
                        coarse >= function.dmx_from.min(function.dmx_to)
                            && coarse <= function.dmx_from.max(function.dmx_to)
                    });
                    let dmx = if channel.fine_offset.is_some() {
                        format!("DMX {raw} ({} / {})", raw >> 8, raw as u8)
                    } else {
                        format!("DMX {coarse}")
                    };
                    lines.push(match current {
                        Some((_, function)) => {
                            format!("  current: {} ({dmx})", function.name)
                        }
                        None => format!("  current: no set ({dmx})"),
                    });
                }
                for number in &missing {
                    lines.push(format!(
                        "Warning: fixture {number} has no `{canonical}` in its mode"
                    ));
                }
            }
            Some(selector) => {
                let selector = match &selector {
                    SetTarget::Index(index) => SetSelector::Index(*index),
                    SetTarget::Name(name) => SetSelector::Name(name),
                };
                let mut assignments = Vec::new();
                let mut chosen = None;
                for (number, channel) in &addressed {
                    let Some((index, function)) = channel.set_for(selector) else {
                        return Err(channel_set_error(*number, &canonical, channel, selector));
                    };
                    let Some(level) = channel.level_for_set(selector) else {
                        continue;
                    };
                    if chosen.is_none() {
                        chosen = Some((index, function.name.clone(), level));
                    }
                    assignments.push((*number, level));
                }
                let Some((index, name, level)) = chosen else {
                    return Err(CommandError::with_hint(
                        format!("no fixture has `{canonical}`"),
                        "`Sheet` lists the patched attributes",
                    ));
                };
                let label = format_fixture_list(&targets);
                let percent = (level * 100.0).round() as i64;
                let message = self.edit_programmer(|programmer| {
                    for (number, level) in &assignments {
                        programmer.set(*number, &canonical, *level);
                    }
                    format!("{label} {canonical} set {index} \"{name}\" at {percent}%")
                });
                self.desk.selection = targets;
                lines.push(message);
                for number in &missing {
                    lines.push(format!(
                        "Warning: fixture {number} has no `{canonical}` in its mode"
                    ));
                }
            }
        }
        Ok(CommandOutcome::with_lines(lines))
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
            self.desk.selection.clear();
            // Dropping a highlight is an output change even though no value
            // was touched.
            if std::mem::take(&mut self.desk.highlight) {
                self.revision += 1;
            }
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
        for (number, attributes) in self.desk.programmer.fixtures() {
            if targets.contains(&number) {
                cleared += attributes.len();
            }
        }
        let message = self.edit_programmer(|programmer| {
            for number in &targets {
                programmer.remove_fixture(*number);
            }
            format!(
                "Cleared {cleared} programmer value{} on {label}",
                if cleared == 1 { "" } else { "s" }
            )
        });
        if all {
            self.desk.selection.clear();
            self.desk.highlight = false;
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
        self.desk.clear();
        self.playback.clear();
        self.effects.stop_all();
        self.timecode.stop();
        self.revision += 1;
        // `New` replaces the document, so replay has to replace it too.
        self.note(crate::journal::EntryKind::Show);
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
            home: Default::default(),
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

    /// `Clone Fixture 1 At 1.101 [Count 8] [Step 4]`: copy a fixture's type,
    /// mode, home and name across the rig. Every new range is checked before
    /// anything is inserted, so a failed clone leaves the patch untouched.
    fn clone_fixtures(
        &mut self,
        source: u16,
        universe: u16,
        address: u16,
        count: Option<u16>,
        step: Option<u16>,
    ) -> Result<CommandOutcome, CommandError> {
        let fixture = self.show.fixture(source).cloned().ok_or_else(|| {
            CommandError::with_hint(
                format!("fixture {source} is not patched"),
                "`Sheet` lists the patched fixtures",
            )
        })?;
        let footprint = self
            .show
            .mode_for(&fixture)
            .map(|(_, mode)| mode.footprint())
            .map_err(CommandError::new)?;
        let step = step.unwrap_or(footprint);
        let count = usize::from(count.unwrap_or(1));
        if self.show.fixtures.len() + count > MAX_FIXTURES {
            return Err(CommandError::new(format!(
                "cloning {count} fixtures would exceed the {MAX_FIXTURES}-fixture limit"
            )));
        }
        let universe_config = self.show.universe(universe).cloned().ok_or_else(|| {
            CommandError::with_hint(
                format!("universe {universe} does not exist"),
                format!(
                    "defined universes: {}",
                    self.show
                        .universes
                        .iter()
                        .map(|universe| universe.number.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        })?;

        let existing: Vec<(u16, PatchRange)> = self
            .show
            .fixtures
            .iter()
            .filter_map(|other| {
                let (_, mode) = self.show.mode_for(other).ok()?;
                PatchRange::new(other.universe, other.address, mode.footprint())
                    .map(|range| (other.number, range))
            })
            .collect();

        let mut used: BTreeSet<u16> = self
            .show
            .fixtures
            .iter()
            .map(|fixture| fixture.number)
            .collect();
        let mut clones = Vec::with_capacity(count);
        let mut placed: Vec<PatchRange> = Vec::new();
        for index in 0..count {
            let offset = u32::from(address) + index as u32 * u32::from(step);
            if offset > u32::from(u16::MAX) {
                return Err(CommandError::new(format!(
                    "clone {} would leave the address range",
                    index + 1
                )));
            }
            let offset = offset as u16;
            let range = PatchRange::new(universe, offset, footprint).ok_or_else(|| {
                CommandError::new(format!(
                    "cannot patch {footprint} channels at {universe}.{offset:03}"
                ))
            })?;
            if !range.fits(universe_config.size) {
                return Err(CommandError::new(format!(
                    "clone {} needs {footprint} slots at {universe}.{offset:03} but universe \
                     {universe} has {} slots",
                    index + 1,
                    universe_config.size
                )));
            }
            if let Some((other, _)) = existing.iter().find(|(_, other)| other.overlaps(&range)) {
                return Err(CommandError::with_hint(
                    format!(
                        "clone {} would overlap fixture {other} at {universe}.{:03}",
                        index + 1,
                        range.start
                    ),
                    "pick another address or a larger step",
                ));
            }
            if let Some(other) = placed.iter().position(|other| other.overlaps(&range)) {
                return Err(CommandError::new(format!(
                    "clone {} would overlap clone {} at {universe}.{:03}",
                    index + 1,
                    other + 1,
                    range.start
                )));
            }
            let number = next_fixture_number(&mut used)?;
            let base = if fixture.name.trim().is_empty() {
                fixture.type_key.clone()
            } else {
                fixture.name.trim().to_string()
            };
            clones.push(Fixture {
                number,
                name: format!("{base} {}", index + 2),
                type_key: fixture.type_key.clone(),
                mode: fixture.mode.clone(),
                universe,
                address: offset,
                home: fixture.home.clone(),
            });
            placed.push(range);
        }

        let first = clones
            .first()
            .map(|fixture| fixture.address)
            .unwrap_or(address);
        let last = clones
            .last()
            .map(|fixture| fixture.address)
            .unwrap_or(address);
        let count = clones.len();
        let message = self.mutate(|show| {
            show.fixtures.extend(clones);
            show.fixtures.sort_by_key(|fixture| fixture.number);
            Ok(format!(
                "Cloned fixture {source} to {count} new fixtures at {universe}.{first:03}..{universe}.{last:03}"
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    /// `Patch Check`: a read-only summary of the whole rig.
    fn patch_check(&self) -> CommandOutcome {
        let mut problems = Vec::new();
        let mut ranges: Vec<(u16, PatchRange)> = Vec::new();
        let mut slots = 0u32;
        let mut used: BTreeMap<u16, u32> = BTreeMap::new();
        for fixture in &self.show.fixtures {
            let (_, mode) = match self.show.mode_for(fixture) {
                Ok(value) => value,
                Err(error) => {
                    problems.push(format!("fixture {}: {error}", fixture.number));
                    continue;
                }
            };
            let footprint = mode.footprint();
            slots += u32::from(footprint);
            let Some(range) = PatchRange::new(fixture.universe, fixture.address, footprint) else {
                problems.push(format!(
                    "fixture {} has an invalid address {}.{:03}",
                    fixture.number, fixture.universe, fixture.address
                ));
                continue;
            };
            let Some(universe) = self.show.universe(fixture.universe) else {
                problems.push(format!(
                    "fixture {} is in undefined universe {}",
                    fixture.number, fixture.universe
                ));
                continue;
            };
            *used.entry(fixture.universe).or_default() += u32::from(footprint);
            if !range.fits(universe.size) {
                problems.push(format!(
                    "fixture {} at {}.{:03} needs {footprint} slots but universe {} has {} slots",
                    fixture.number,
                    fixture.universe,
                    fixture.address,
                    fixture.universe,
                    universe.size
                ));
            }
            ranges.push((fixture.number, range));
        }

        let universes: Vec<u16> = used.keys().copied().collect();
        let fixtures = self.show.fixtures.len();
        let mut lines = vec![format!(
            "{fixtures} fixture{}, {slots} slot{}, universes {}",
            if fixtures == 1 { "" } else { "s" },
            if slots == 1 { "" } else { "s" },
            if universes.is_empty() {
                "none".to_string()
            } else {
                universes
                    .iter()
                    .map(|number| number.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )];
        let conflicts = patch::conflicts(&ranges);
        if conflicts.is_empty() {
            lines.push("no overlaps".into());
        }
        for (first, second) in conflicts {
            lines.push(format!("overlap: fixtures {first} and {second}"));
        }

        for universe in &self.show.universes {
            if !universes.contains(&universe.number) {
                continue;
            }
            let in_universe: Vec<PatchRange> = ranges
                .iter()
                .filter(|(_, range)| range.universe == universe.number)
                .map(|(_, range)| *range)
                .collect();
            let gaps = patch::gaps(&in_universe, universe.size, 8);
            let usage = format!(
                "universe {}: {} of {} slots used",
                universe.number,
                used.get(&universe.number).copied().unwrap_or(0),
                universe.size
            );
            let largest = gaps.iter().copied().max_by_key(|(_, length)| *length);
            match largest {
                Some((start, length)) => {
                    lines.push(format!(
                        "{usage}, largest gap {length} slots at {}.{:03}",
                        universe.number, start
                    ));
                    for (other_start, other_length) in &gaps {
                        if Some((*other_start, *other_length)) != largest {
                            lines.push(format!(
                                "  gap {other_length} slots at {}.{:03}",
                                universe.number, other_start
                            ));
                        }
                    }
                }
                None => lines.push(usage),
            }
        }
        for problem in problems {
            lines.push(format!("Warning: {problem}"));
        }
        CommandOutcome::with_lines(lines)
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
        self.desk
            .selection
            .retain(|number| !fixtures.contains(number));
        Ok(CommandOutcome::message(message))
    }

    /// `Patch Import "rig.csv"`: replace the patch from a spreadsheet.
    ///
    /// The whole file is checked before `mutate` sees it, so a bad row leaves
    /// the existing patch exactly as it was.
    fn import_patch(&mut self, path: &str) -> Result<CommandOutcome, CommandError> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            CommandError::with_hint(
                format!("cannot read `{path}`: {error}"),
                "`Report Patch` writes a file in this format",
            )
        })?;
        let parsed = csv::parse_patch(&text).map_err(CommandError::new)?;

        let mut fixtures = Vec::with_capacity(parsed.rows.len());
        let mut numbers = BTreeSet::new();
        let mut placed: Vec<(u16, PatchRange)> = Vec::new();
        for (index, row) in parsed.rows.iter().enumerate() {
            let line = index + 2;
            if !numbers.insert(row.number) {
                return Err(CommandError::with_hint(
                    format!("line {line}: fixture number {} appears twice", row.number),
                    "each row needs a unique fixture number",
                ));
            }
            let fixture_type = self.show.find_type(&row.type_key).ok_or_else(|| {
                CommandError::with_hint(
                    format!("line {line}: unknown fixture type `{}`", row.type_key),
                    "import the fixture type first: `Import Gdtf \"file.gdtf\"`",
                )
            })?;
            let mode = fixture_type.mode(&row.mode).ok_or_else(|| {
                CommandError::with_hint(
                    format!(
                        "line {line}: `{}` has no mode `{}`",
                        fixture_type.key(),
                        row.mode
                    ),
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
            })?;
            let universe = self.show.universe(row.universe).ok_or_else(|| {
                CommandError::with_hint(
                    format!("line {line}: universe {} does not exist", row.universe),
                    format!(
                        "defined universes: {}",
                        self.show
                            .universes
                            .iter()
                            .map(|universe| universe.number.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?;
            let range =
                PatchRange::new(row.universe, row.address, mode.footprint()).ok_or_else(|| {
                    CommandError::new(format!(
                        "line {line}: cannot patch {} channels at {}.{:03}",
                        mode.footprint(),
                        row.universe,
                        row.address
                    ))
                })?;
            if !range.fits(universe.size) {
                return Err(CommandError::new(format!(
                    "line {line}: fixture needs {} channels at {}.{:03} but universe {} has {} slots",
                    mode.footprint(),
                    row.universe,
                    row.address,
                    row.universe,
                    universe.size
                )));
            }
            if let Some((other, _)) = placed.iter().find(|(_, other)| other.overlaps(&range)) {
                return Err(CommandError::new(format!(
                    "line {line}: fixture {} overlaps fixture {other} at {}.{:03}",
                    row.number, row.universe, range.start
                )));
            }
            placed.push((row.number, range));
            fixtures.push(Fixture {
                number: row.number,
                name: row.name.clone(),
                type_key: fixture_type.key(),
                mode: mode.name.clone(),
                universe: row.universe,
                address: row.address,
                home: Default::default(),
            });
        }
        fixtures.sort_by_key(|fixture| fixture.number);
        let count = fixtures.len();
        let message = self.mutate(|show| {
            show.fixtures = fixtures;
            Ok(format!("Imported {count} fixtures from {path}"))
        })?;
        let live: BTreeSet<u16> = self
            .show
            .fixtures
            .iter()
            .map(|fixture| fixture.number)
            .collect();
        self.desk.selection.retain(|number| live.contains(number));

        let mut lines = vec![message];
        lines.extend(
            parsed
                .warnings
                .into_iter()
                .map(|warning| format!("Warning: {warning}")),
        );
        Ok(CommandOutcome::with_lines(lines))
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

    /// Write the desk and/or the show. The desk goes first: if it fails the
    /// show is untouched, so a document can never claim a newer state than its
    /// desk.
    fn save(&mut self, path: &str, desk: bool, show: bool) -> Result<CommandOutcome, CommandError> {
        let show_path = Path::new(path);
        let mut lines = Vec::new();
        if desk {
            self.sync_desk_runs();
            let desk_path = Desk::path_for(show_path);
            self.desk
                .save(&desk_path, &self.show)
                .map_err(|error| CommandError::new(error.to_string()))?;
            lines.push(format!("Saved desk to {}", desk_path.display()));
        }
        if show {
            self.show
                .save(show_path)
                .map_err(|error| CommandError::new(error.to_string()))?;
            // The file now contains every statement so far, so replay starts
            // after this point rather than from the beginning of the journal.
            if let Some(journal) = &mut self.journal {
                journal.checkpoint(show_path, self.revision);
            }
            lines.push(format!("Saved show to {path}"));
        }
        if let Err(error) = self.autosave_now() {
            lines.push(format!("Warning: autosave failed: {error}"));
        }
        Ok(CommandOutcome::with_lines(lines))
    }

    fn load(&mut self, path: &str, show_only: bool) -> Result<CommandOutcome, CommandError> {
        let show_path = Path::new(path);
        let (show, report) = Show::load_with_report(show_path)
            .map_err(|error| CommandError::new(error.to_string()))?;
        let mut lines = vec![format!(
            "Loaded `{}` ({} fixtures, {} fixture types) from {path}",
            show.name,
            show.fixtures.len(),
            show.fixture_types.len()
        )];
        self.show = show;
        // A loaded show has its own cues and pools; runs from the previous show
        // must not keep writing the old values over the new rig.
        self.playback.clear();
        self.effects.stop_all();
        self.timecode.stop();
        self.history.clear();
        self.desk.clear();

        let desk_path = Desk::path_for(show_path);
        let loaded = if show_only {
            lines.extend(self.desk.sanitise(&self.show));
            None
        } else {
            Desk::load(&desk_path, &self.show)
                .map_err(|error| CommandError::new(error.to_string()))?
        };
        match loaded {
            Some((desk, warnings)) => {
                self.desk = desk;
                lines.extend(
                    warnings
                        .into_iter()
                        .map(|warning| format!("Warning: {warning}")),
                );
            }
            None => self.apply_seed(report.desk_seed.clone()),
        }
        lines.extend(
            report
                .warnings
                .into_iter()
                .map(|warning| format!("Warning: {warning}")),
        );
        self.restore_desk_runs();
        self.revision += 1;
        Ok(CommandOutcome::with_lines(lines))
    }

    /// Load a show and the desk that sits beside it.
    ///
    /// A show without a desk file opens clean, and a show that was upgraded
    /// from v4 seeds the desk with the programmer it used to carry.
    pub fn open(path: &Path) -> Result<(Engine, Vec<String>), String> {
        let (show, report) = Show::load_with_report(path).map_err(|error| error.to_string())?;
        let mut warnings = report.warnings.clone();
        let mut engine = Engine::new(show);
        let desk_path = Desk::path_for(path);
        match Desk::load(&desk_path, &engine.show).map_err(|error| error.to_string())? {
            Some((desk, desk_warnings)) => {
                engine.desk = desk;
                warnings.extend(desk_warnings);
            }
            None => engine.apply_seed(report.desk_seed),
        }
        engine.restore_desk_runs();
        Ok((engine, warnings))
    }

    /// Move values an old show file carried into this desk.
    fn apply_seed(&mut self, seed: crate::show::DeskSeed) {
        if let Some(grand) = seed.grand {
            self.desk.masters.grand = grand;
        }
        if let Some(rate) = seed.rate {
            self.desk.masters.rate = rate;
        }
        for (number, level) in seed.groups {
            self.desk.masters.set_group(number, level);
        }
        for (number, level) in seed.executors {
            self.desk.set_executor_fader(number, level);
        }
        self.desk.programmer = seed.programmer;
    }

    /// Record which cue every playback is on, for the desk file.
    fn sync_desk_runs(&mut self) {
        self.desk.runs = self
            .playback
            .states()
            .into_iter()
            .map(|(target, cue, paused)| {
                let (sequence, executor) = match target {
                    Target::Sequence(number) => (Some(number), None),
                    Target::Executor(number) => (None, Some(number)),
                };
                RunState {
                    sequence,
                    executor,
                    cue,
                    paused,
                }
            })
            .collect();
    }

    /// Start the playbacks a desk file recorded, at the cue it recorded.
    ///
    /// A run always starts its fade from the current output, so a desk coming
    /// back up fades in rather than jumping.
    fn restore_desk_runs(&mut self) {
        let runs = std::mem::take(&mut self.desk.runs);
        for run in &runs {
            let target = match (run.executor, run.sequence) {
                (Some(executor), _) => Target::Executor(executor),
                (None, Some(sequence)) => Target::Sequence(sequence),
                (None, None) => continue,
            };
            let Ok(sequence_number) = self.sequence_number_for(target) else {
                continue;
            };
            let Some(index) = self
                .show
                .sequence(sequence_number)
                .and_then(|sequence| sequence.cue_index(run.cue))
            else {
                continue;
            };
            let _ = self
                .playback
                .go(&self.show, target, sequence_number, index, self.clock);
            if run.paused {
                self.playback.pause(target, self.clock);
            }
        }
        self.playback.refresh(&self.desk, self.clock);
        self.desk.runs = runs;
    }

    fn sheet(&self) -> CommandOutcome {
        if self.show.fixtures.is_empty() {
            return CommandOutcome::message("No fixtures patched.");
        }
        let mut lines = Vec::new();
        if let Some(banner) = self.preview_banner() {
            lines.push(banner);
        }
        lines.push(format!(
            "{:<4} {:<22} {:<26} {:<10} {:<10} {}",
            "No.", "Name", "Type", "Mode", "Patch", "Output"
        ));
        let mut fixtures: Vec<&Fixture> = self.show.fixtures.iter().collect();
        fixtures.sort_by_key(|fixture| fixture.number);
        let playback = self.playback.output();
        let effects = self
            .effects
            .output(&self.show, self.desk.masters.rate, self.clock);
        let stage = Stage::new(
            &self.show,
            &self.desk.programmer,
            playback,
            &effects,
            &self.desk.masters,
            self.desk.blackout_master(self.clock),
        );

        for fixture in fixtures {
            let (patch, levels) = match self.show.mode_for(fixture) {
                Ok((fixture_type, mode)) => {
                    let mut seen = BTreeSet::new();
                    let mut parts = Vec::new();
                    for channel in &mode.channels {
                        if !seen.insert(channel.attribute.to_ascii_lowercase()) {
                            continue;
                        }
                        // The sheet shows what the rig is doing and where the
                        // base value came from: P, Pb, H or nothing.
                        let source = stage.source(fixture, &channel.attribute);
                        let level = stage.level(fixture, &channel.attribute);
                        parts.push(format!(
                            "{} {} {}%",
                            source.label(),
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
                "Output: {}   engine {:.1} Hz   frame {:.1} Hz target",
                if status.running { "running" } else { "stopped" },
                status.engine_hz,
                status.frame_hz
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
                .map(|error| format!("  last error: {error}"))
                .unwrap_or_default();
            lines.push(format!(
                "Universe {universe}  {:<24} {:<12} {:>5.1} Hz  frames {}  errors {}  reconnects {}  \
                 send {} us  stale {} ms{last_error}",
                entry.transport,
                entry.state.label(),
                entry.achieved_hz,
                entry.frames,
                entry.errors,
                entry.reconnects,
                entry.last_send_us,
                entry.stale_ms
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
            if self.desk.selection.is_empty() {
                "none".to_string()
            } else {
                format_fixture_list(&self.desk.selection)
            }
        ));
        lines.push(format!(
            "Programmer: {}",
            count(self.desk.programmer.len(), "value")
        ));
        lines.push(format!(
            "Palettes: {} | Sequences: {} | Executors: {}",
            self.show.palettes.len(),
            self.show.sequences.len(),
            self.show.executors.len()
        ));
        lines.push(format!(
            "Groups: {} | Worlds: {} | Filters: {} | Effects: {}",
            self.show.groups.len(),
            self.show.worlds.len(),
            self.show.filters.len(),
            self.show.effects.len()
        ));
        lines.push(format!(
            "Masters: grand {}% | rate {}%",
            (self.desk.masters.grand * 100.0).round() as i64,
            (self.desk.masters.rate * 100.0).round() as i64
        ));
        lines.push(format!(
            "Blind: {} | Highlight: {} | Preview: {}",
            if self.desk.blind { "on" } else { "off" },
            if self.desk.highlight { "on" } else { "off" },
            self.preview_summary()
        ));
        lines.push(format!(
            "Blackout: {} | Lock: {}",
            match self.desk.blackout {
                Some(state) => format!(
                    "on ({}%, fade {:.1}s)",
                    (self.desk.blackout_master(self.clock) * 100.0).round() as i64,
                    state.fade.as_secs_f64()
                ),
                None => "off".to_string(),
            },
            if self.desk.lock.is_some() {
                "on"
            } else {
                "off"
            }
        ));
        lines.push(format!(
            "Solo: {}",
            self.desk
                .soloed()
                .map(|number| format!("executor {number}"))
                .unwrap_or_else(|| "off".to_string())
        ));
        let autosave = match &self.autosave {
            Some((path, interval, _)) => format!(
                "every {}s -> {}{}",
                interval.as_secs(),
                path.display(),
                match &self.autosave_error {
                    Some(error) => format!(" (failed: {error})"),
                    None => String::new(),
                }
            ),
            None => "off".to_string(),
        };
        lines.push(match &self.journal {
            Some(journal) => format!(
                "Journal: {} ({} entries{}) | Autosave: {autosave}",
                journal.path().display(),
                journal.entries(),
                match journal.failure() {
                    Some(error) => format!(", stopped: {error}"),
                    None => String::new(),
                }
            ),
            None => format!("Journal: off | Autosave: {autosave}"),
        });
        lines.push(format!(
            "World: {} | Filter: {}",
            self.desk
                .world
                .and_then(|number| self.show.world(number))
                .map(|world| world.display_name())
                .unwrap_or_else(|| "off".to_string()),
            self.desk
                .filter
                .and_then(|number| self.show.filter(number))
                .map(|filter| filter.display_name())
                .unwrap_or_else(|| "off".to_string())
        ));
        if self.effects.is_running() {
            for (number, fixtures) in self.effects.runs() {
                lines.push(format!(
                    "Effect {number}: running on {}",
                    format_fixture_list(fixtures)
                ));
            }
        } else {
            lines.push("Effects: idle".to_string());
        }
        lines.push(format!(
            "Undo: {} | Redo: {}",
            count(self.history.undo_depth(), "step"),
            count(self.history.redo_depth(), "step")
        ));
        lines.push(format!("Timecode: {}", self.timecode_status()));
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
        if self.desk.selection.is_empty() {
            let all: Vec<u16> = self
                .show
                .fixtures
                .iter()
                .map(|fixture| fixture.number)
                .filter(|number| self.world_allows(*number))
                .collect();
            if all.is_empty() {
                return Err(CommandError::with_hint(
                    "nothing to store",
                    if self.show.fixtures.is_empty() {
                        "patch a fixture first: `Patch Fixture 1 At 1.001 Type \"…\"`"
                    } else {
                        "the active world leaves no patched fixtures; deactivate it with `World All`"
                    },
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
        if let Some(filter) = self.active_filter() {
            if !filter.allows(feature) {
                return Err(CommandError::with_hint(
                    format!("`{feature}` is outside {}", filter.display_name()),
                    format!("the filter allows: {}", filter.features.join(", ")),
                ));
            }
        }
        let (pool, values) = palette::capture(&self.show, &self.desk.programmer, &targets, feature)
            .map_err(CommandError::new)?;
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

        if let Some(filter) = self.active_filter() {
            if !filter.allows(&palette.feature) {
                return Err(CommandError::with_hint(
                    format!(
                        "palette {feature} {number} is outside {}",
                        filter.display_name()
                    ),
                    format!("the filter allows: {}", filter.features.join(", ")),
                ));
            }
        }

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
        let message = self.edit_programmer(|programmer| {
            for (number, values) in &assignments {
                for (attribute, level) in values {
                    programmer.set(*number, attribute, *level);
                }
            }
            format!(
                "Applied palette {label} to {applied} fixture{} ({value_count} values)",
                if applied == 1 { "" } else { "s" }
            )
        });

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

    /// `Report <kind> [<sequence>] <file>`: write what the listings already say.
    /// Reports are read-only: no journal entry, no undo step, no revision bump.
    fn report(
        &self,
        kind: ReportKind,
        sequence: Option<u16>,
        path: &str,
    ) -> Result<CommandOutcome, CommandError> {
        let (label, text) = match kind {
            ReportKind::Patch => ("patch", csv::write_patch(&self.patch_rows()?)),
            ReportKind::Cues => ("cues", self.cues_report(sequence, path)?),
            ReportKind::Palettes => ("palettes", text_of(self.palette_list())),
            ReportKind::Status => ("status", text_of(self.status_report())),
            ReportKind::Show => ("show", self.show_report()),
        };
        crate::store::write_atomic(Path::new(path), text.as_bytes())
            .map_err(|error| CommandError::new(format!("cannot write `{path}`: {error}")))?;
        Ok(CommandOutcome::message(format!(
            "Wrote {label} report to {path}"
        )))
    }

    /// One row per fixture in fixture-number order, the same columns import
    /// reads.
    fn patch_rows(&self) -> Result<Vec<csv::PatchRow>, CommandError> {
        let mut fixtures: Vec<&Fixture> = self.show.fixtures.iter().collect();
        fixtures.sort_by_key(|fixture| fixture.number);
        let mut rows = Vec::with_capacity(fixtures.len());
        for fixture in fixtures {
            self.show.mode_for(fixture).map_err(|error| {
                CommandError::new(format!("fixture {}: {error}", fixture.number))
            })?;
            rows.push(csv::PatchRow {
                number: fixture.number,
                name: fixture.name.clone(),
                type_key: fixture.type_key.clone(),
                mode: fixture.mode.clone(),
                universe: fixture.universe,
                address: fixture.address,
            });
        }
        Ok(rows)
    }

    /// A cues report is CSV when the file ends in `.csv`, and the ordinary
    /// listing otherwise. Cue values are never part of it: this is the list as
    /// data, not a way to move programming between shows.
    fn cues_report(&self, sequence: Option<u16>, path: &str) -> Result<String, CommandError> {
        let sequences: Vec<&Sequence> = match sequence {
            Some(number) => vec![self.show.sequence(number).ok_or_else(|| {
                CommandError::with_hint(
                    format!("sequence {number} does not exist"),
                    "`Sequence` lists the stored cue lists",
                )
            })?],
            None => self.show.sequences.iter().collect(),
        };
        if path.to_ascii_lowercase().ends_with(".csv") {
            let mut rows = Vec::new();
            for sequence in &sequences {
                for cue in &sequence.cues {
                    rows.push(vec![
                        sequence.number.to_string(),
                        format_cue_number(cue.number),
                        cue.name.clone(),
                        format!("{:.1}", cue.fade),
                        format!("{:.1}", cue.delay),
                        format_trigger(cue.trigger, cue.trigger_time),
                        if cue.mib { "yes" } else { "no" }.to_string(),
                        if cue.blocked { "yes" } else { "no" }.to_string(),
                    ]);
                }
            }
            return Ok(csv::write_records(
                &[
                    "sequence", "number", "name", "fade", "delay", "trigger", "mib", "blocked",
                ],
                &rows,
            ));
        }
        if sequences.is_empty() {
            return Ok(text_of(self.sequence_list(None)));
        }
        let mut lines = Vec::new();
        for sequence in sequences {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(self.sequence_list(Some(sequence.number)).lines);
        }
        Ok(lines.join("\n") + "\n")
    }

    /// Every listing the shell shows, in window order, as one Markdown file.
    fn show_report(&self) -> String {
        let playback = join_sections(vec![
            self.sequence_list(None).lines,
            self.executor_list(None).lines,
        ]);
        let pools = join_sections(vec![
            self.group_list().lines,
            self.world_list().lines,
            self.filter_list().lines,
            self.effect_list(None).lines,
        ]);
        let sections = [
            ("Sheet", self.sheet().lines),
            ("Playback", playback),
            ("Pools", pools),
            ("Timecode", self.timecode_list(None).lines),
            ("Status", self.status_report().lines),
        ];
        let mut text = format!("# {}\n", self.show.name);
        for (title, lines) in sections {
            text.push_str(&format!("\n## {title}\n\n"));
            text.push_str(&lines.join("\n"));
            text.push('\n');
        }
        text
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

    fn store_cue(&mut self, spec: CueSpec) -> Result<CommandOutcome, CommandError> {
        let CueSpec {
            sequence: sequence_number,
            number,
            name,
            fade,
            delay,
            mib,
            trigger,
            mode,
        } = spec;
        if sequence_number == 0 {
            return Err(CommandError::new("sequence number 0 is not allowed"));
        }
        if !number.is_finite() || !(0.0..=MAX_CUE_NUMBER).contains(&number) {
            return Err(CommandError::new(format!(
                "cue number {number} is outside 0..={MAX_CUE_NUMBER}"
            )));
        }
        for (label, value) in [
            ("fade", fade),
            ("delay", delay),
            ("trigger time", trigger.map(|trigger| trigger.time)),
        ] {
            if let Some(value) = value {
                if !value.is_finite() || !(0.0..=MAX_TIME).contains(&value) {
                    return Err(CommandError::new(format!(
                        "{label} {value} is outside 0..={MAX_TIME} seconds"
                    )));
                }
            }
        }

        let captured = self.capture_programmer()?;

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
            let existing_mib = existing
                .and_then(|index| sequence.cues.get(index))
                .map(|cue| cue.mib)
                .unwrap_or(false);
            match existing {
                Some(index) => {
                    let cue = &mut sequence.cues[index];
                    if name.is_some() {
                        cue.name = cue_name.clone();
                    }
                    cue.fade = fade;
                    cue.delay = delay;
                    if let Some(mib) = mib {
                        cue.mib = mib;
                    }
                    if let Some(trigger) = trigger {
                        apply_trigger(cue, trigger);
                    }
                    let blocked = cue.blocked;
                    match mode {
                        StoreMode::Merge => {
                            if blocked {
                                // A blocked cue holds the whole state, so the
                                // programmer lands on the state before it.
                                let mut absolute = previous.clone();
                                absolute.overlay(&captured);
                                cue.levels = absolute;
                            } else {
                                cue.levels.overlay(&captured);
                            }
                        }
                        StoreMode::Replace => {
                            cue.levels = if blocked {
                                captured.clone()
                            } else {
                                captured.difference(&previous)
                            };
                        }
                    }
                    sequence.rebase();
                    values = sequence.cues[index].levels.len();
                }
                None => {
                    let levels = captured.difference(&previous);
                    values = levels.len();
                    let mut cue = Cue {
                        number,
                        name: cue_name.clone(),
                        fade,
                        delay,
                        mib: mib.unwrap_or(false),
                        trigger: Trigger::Go,
                        trigger_time: 0.0,
                        timing: Default::default(),
                        stagger: 0.0,
                        blocked: false,
                        shield: Default::default(),
                        levels,
                    };
                    if let Some(trigger) = trigger {
                        apply_trigger(&mut cue, trigger);
                    }
                    sequence.cues.insert(insertion, cue);
                    sequence.rebase();
                }
            }
            show.sequences.sort_by_key(|sequence| sequence.number);
            let mib_label = if mib.unwrap_or(existing_mib) { ", mib" } else { "" };
            Ok(format!(
                "{} cue {}{name_label} in sequence {sequence_number} (fade {}s, delay {}s{mib_label}, {values} tracked values)",
                if created {
                    "Stored"
                } else if mode == StoreMode::Replace {
                    "Updated (replace)"
                } else {
                    "Updated (merge)"
                },
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

    /// The programmer values a store or update captures, through the active
    /// filter. Selection and world rules are the same as `Store`'s.
    fn capture_programmer(&self) -> Result<Levels, CommandError> {
        let targets = self.programmer_targets()?;
        let mut captured = Levels::new();
        for target in &targets {
            let Some(fixture) = self.show.fixture(*target) else {
                continue;
            };
            let Ok((fixture_type, _)) = self.show.mode_for(fixture) else {
                continue;
            };
            for (number, attributes) in self.desk.programmer.fixtures() {
                if number != *target {
                    continue;
                }
                for (attribute, level) in attributes {
                    if self.attribute_allowed(fixture_type, attribute) {
                        captured.set(*target, attribute, *level);
                    }
                }
            }
        }
        Ok(captured)
    }

    /// `Update [Cue <n>] [Sequence <n>]`: merge the programmer's touched
    /// attributes into a cue, defaulting to the cue that sequence is running.
    /// One undo step, like every other show edit.
    fn update_cue(
        &mut self,
        sequence_number: u16,
        number: Option<f64>,
    ) -> Result<CommandOutcome, CommandError> {
        if sequence_number == 0 {
            return Err(CommandError::new("sequence number 0 is not allowed"));
        }
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "`Sequence` lists the sequences",
            ));
        };
        let number = match number {
            Some(number) => number,
            None => self.running_cue_number(sequence_number).ok_or_else(|| {
                CommandError::with_hint(
                    format!("sequence {sequence_number} is not running"),
                    "name the cue to update: `Update Cue 2`",
                )
            })?,
        };
        let label = format_cue_number(number);
        let Some(index) = sequence.cue_index(number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} has no cue {label}"),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        };
        let captured = self.capture_programmer()?;
        if captured.is_empty() {
            return Err(CommandError::with_hint(
                "the programmer is empty",
                "select a fixture and give it a level first",
            ));
        }
        let absolute = sequence.state_at(index);
        let changed = captured
            .fixtures()
            .map(|(fixture, attributes)| {
                attributes
                    .iter()
                    .filter(|(attribute, level)| absolute.get(fixture, attribute) != Some(**level))
                    .count()
            })
            .sum::<usize>();
        let message = self.mutate(|show| {
            let message = format!(
                "Updated cue {label} in sequence {sequence_number} ({changed} values changed)"
            );
            let Some(sequence_index) = show.sequence_index(sequence_number) else {
                return Ok(message);
            };
            let sequence = &mut show.sequences[sequence_index];
            // Merge by attribute: a blocked cue already holds the whole state,
            // so overlaying the programmer is the same edit there.
            if let Some(cue) = sequence.cues.get_mut(index) {
                cue.levels.overlay(&captured);
            }
            sequence.rebase();
            Ok(message)
        })?;
        Ok(CommandOutcome::message(message))
    }

    /// The cue a sequence is running, whether it was started directly or
    /// through an executor assigned to it.
    fn running_cue_number(&self, sequence_number: u16) -> Option<f64> {
        self.playback.runs().find_map(|(target, run)| {
            let matches = match target {
                Target::Sequence(number) => number == sequence_number,
                Target::Executor(number) => {
                    self.show.executor(number).map(|executor| executor.sequence)
                        == Some(sequence_number)
                }
            };
            matches.then_some(run.cue_number)
        })
    }

    /// `Cue <n> Block` / `Cue <n> Unblock`: stop or resume tracking at a cue.
    /// Blocking materialises the state the cue leaves behind, so later edits to
    /// earlier cues no longer move it; unblocking turns it back into a delta.
    fn cue_block(
        &mut self,
        sequence_number: u16,
        number: f64,
        on: bool,
    ) -> Result<CommandOutcome, CommandError> {
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "`Sequence` lists the sequences",
            ));
        };
        let label = format_cue_number(number);
        let Some(index) = sequence.cue_index(number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} has no cue {label}"),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        };
        let message = self.mutate(|show| {
            let Some(sequence_index) = show.sequence_index(sequence_number) else {
                return Ok(String::new());
            };
            let sequence = &mut show.sequences[sequence_index];
            if on {
                let state = sequence.state_at(index);
                if let Some(cue) = sequence.cues.get_mut(index) {
                    cue.levels = state;
                    cue.blocked = true;
                }
            } else if let Some(cue) = sequence.cues.get_mut(index) {
                cue.blocked = false;
            }
            sequence.rebase();
            Ok(if on {
                format!("Cue {label} in sequence {sequence_number} blocks tracking: it holds the whole state")
            } else {
                format!("Cue {label} in sequence {sequence_number} tracks again")
            })
        })?;
        Ok(CommandOutcome::message(message))
    }

    /// `Cue <n> Shield <attribute> [Off]`: assert an attribute at the level it
    /// has at this cue, or remove the assertion. With no attribute it lists the
    /// shields, so one is never an invisible flag.
    fn cue_shield(
        &mut self,
        sequence_number: u16,
        number: f64,
        attribute: Option<String>,
        off: bool,
    ) -> Result<CommandOutcome, CommandError> {
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "`Sequence` lists the sequences",
            ));
        };
        let label = format_cue_number(number);
        let Some(index) = sequence.cue_index(number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} has no cue {label}"),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        };
        let Some(typed) = attribute else {
            let cue = &sequence.cues[index];
            if cue.shield.is_empty() {
                return Ok(CommandOutcome::message(format!(
                    "Cue {label} in sequence {sequence_number}: no shields."
                )));
            }
            let mut lines = vec![format!(
                "Cue {label} in sequence {sequence_number}: {} shields",
                cue.shield.len()
            )];
            lines.push(format!("{:<16} {:>6}", "Attribute", "Level"));
            for (attribute, level) in &cue.shield {
                lines.push(format!(
                    "{:<16} {:>5.0}%",
                    truncate(attribute, 16),
                    level * 100.0
                ));
            }
            return Ok(CommandOutcome::with_lines(lines));
        };
        let canonical = self.resolve_cue_attribute(sequence_number, index, &typed)?;
        if off {
            let message = self.mutate(|show| {
                let Some(sequence_index) = show.sequence_index(sequence_number) else {
                    return Ok(String::new());
                };
                show.sequences[sequence_index].remove_shield(index, &canonical);
                Ok(format!(
                    "Cue {label} in sequence {sequence_number}: `{canonical}` follows the cue again"
                ))
            })?;
            return Ok(CommandOutcome::message(message));
        }

        let state = sequence.state_at(index);
        let values: Vec<f64> = state
            .fixtures()
            .filter_map(|(_, attributes)| {
                attributes
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(&canonical))
                    .map(|(_, level)| *level)
            })
            .collect();
        if values.is_empty() {
            return Err(CommandError::with_hint(
                format!("cue {label} has no `{canonical}` level to shield"),
                "give the attribute a value in an earlier cue, or block the cue first",
            ));
        }
        let level = values[0];
        if values.iter().any(|value| (value - level).abs() > 1e-9) {
            return Err(CommandError::with_hint(
                format!("the fixtures at cue {label} have different `{canonical}` levels"),
                "a shield asserts one level for every fixture; set them equal first",
            ));
        }
        let message = self.mutate(|show| {
            let Some(sequence_index) = show.sequence_index(sequence_number) else {
                return Ok(String::new());
            };
            let sequence = &mut show.sequences[sequence_index];
            if let Some(cue) = sequence.cues.get_mut(index) {
                let key = cue
                    .shield
                    .keys()
                    .find(|name| name.eq_ignore_ascii_case(&canonical))
                    .cloned();
                if let Some(key) = key {
                    cue.shield.remove(&key);
                }
                cue.shield.insert(canonical.clone(), level);
            }
            sequence.rebase();
            Ok(format!(
                "Cue {label} in sequence {sequence_number}: `{canonical}` is shielded at {}%",
                (level * 100.0).round() as i64
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    /// `Cue <n> Show`: the state the cue actually produces, not its stored
    /// delta. This is the question an operator has, and the delta cannot be
    /// read by eye.
    fn cue_show(&self, sequence_number: u16, number: f64) -> Result<CommandOutcome, CommandError> {
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "`Sequence` lists the sequences",
            ));
        };
        let label = format_cue_number(number);
        let Some(index) = sequence.cue_index(number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} has no cue {label}"),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        };
        let cue = &sequence.cues[index];
        let state = sequence.state_at(index);
        let mut lines = vec![format!(
            "Cue {label} in sequence {sequence_number}: absolute state (fade {:.1}s, delay {:.1}s{})",
            cue.fade,
            cue.delay,
            if cue.blocked { ", blocked" } else { "" }
        )];
        if state.is_empty() {
            lines.push("No values.".to_string());
            return Ok(CommandOutcome::with_lines(lines));
        }
        lines.push(format!("{:<7} {}", "Fixture", "Values"));
        for (fixture, attributes) in state.fixtures() {
            let mut parts: Vec<String> = attributes
                .iter()
                .map(|(attribute, level)| {
                    format!("{attribute} {}%", (level * 100.0).round() as i64)
                })
                .collect();
            parts.sort();
            lines.push(format!("{:<7} {}", fixture, parts.join("  ")));
        }
        if !cue.shield.is_empty() {
            let shields: Vec<String> = cue
                .shield
                .iter()
                .map(|(attribute, level)| {
                    format!("{attribute} {}%", (level * 100.0).round() as i64)
                })
                .collect();
            lines.push(format!("Shields: {}", shields.join(", ")));
        }
        Ok(CommandOutcome::with_lines(lines))
    }

    /// `Cue <n> Move <m>`: renumber a cue in place. The cue numbers, not the
    /// positions, are what playback references, so the cue keeps its look and
    /// its timing; `rebase` keeps every other cue where it was.
    fn cue_move(
        &mut self,
        sequence_number: u16,
        number: f64,
        to: f64,
    ) -> Result<CommandOutcome, CommandError> {
        if !to.is_finite() || !(0.0..=MAX_CUE_NUMBER).contains(&to) {
            return Err(CommandError::new(format!(
                "cue number {to} is outside 0..={MAX_CUE_NUMBER}"
            )));
        }
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "`Sequence` lists the sequences",
            ));
        };
        let label = format_cue_number(number);
        let target_label = format_cue_number(to);
        if sequence.cue_index(number).is_none() {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} has no cue {label}"),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        }
        if (number - to).abs() < 1e-9 {
            return Ok(CommandOutcome::message(format!(
                "Cue {label} is already cue {target_label} in sequence {sequence_number}"
            )));
        }
        if sequence.cue_index(to).is_some() {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} already has cue {target_label}"),
                "move it to a free number, or delete the cue first",
            ));
        }
        let message = self.mutate(|show| {
            let Some(sequence_index) = show.sequence_index(sequence_number) else {
                return Ok(String::new());
            };
            let sequence = &mut show.sequences[sequence_index];
            let Some(position) = sequence.cue_index(number) else {
                return Ok(String::new());
            };
            let mut cue = sequence.cues.remove(position);
            cue.number = to;
            let insertion = sequence.insertion_index(to);
            sequence.cues.insert(insertion, cue);
            sequence.rebase();
            Ok(format!(
                "Moved cue {label} to {target_label} in sequence {sequence_number}"
            ))
        })?;
        // A preview follows the cue it names, even when the cue is renumbered.
        if let Some(preview) = &mut self.desk.preview {
            if preview.sequence == sequence_number && (preview.cue - number).abs() < 1e-9 {
                preview.cue = to;
            }
        }
        Ok(CommandOutcome::message(message))
    }

    /// `Cue <n> Copy <m> [Sequence <s>]`: deep-copy a cue, timing and shields
    /// included. Copying into another sequence rebases both lists.
    fn cue_copy(
        &mut self,
        sequence_number: u16,
        number: f64,
        to: f64,
        target: Option<u16>,
    ) -> Result<CommandOutcome, CommandError> {
        if !to.is_finite() || !(0.0..=MAX_CUE_NUMBER).contains(&to) {
            return Err(CommandError::new(format!(
                "cue number {to} is outside 0..={MAX_CUE_NUMBER}"
            )));
        }
        let target_sequence = target.unwrap_or(sequence_number);
        let Some(source) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "`Sequence` lists the sequences",
            ));
        };
        let label = format_cue_number(number);
        let target_label = format_cue_number(to);
        if source.cue_index(number).is_none() {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} has no cue {label}"),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        }
        let Some(destination) = self.show.sequence(target_sequence) else {
            return Err(CommandError::with_hint(
                format!("sequence {target_sequence} does not exist"),
                format!("store a cue there first: `Store Cue 1 Sequence {target_sequence}`"),
            ));
        };
        if destination.cue_index(to).is_some() {
            return Err(CommandError::with_hint(
                format!("sequence {target_sequence} already has cue {target_label}"),
                "copy it to a free number, or delete the cue first",
            ));
        }
        let message = self.mutate(|show| {
            let Some(source_index) = show.sequence_index(sequence_number) else {
                return Ok(String::new());
            };
            let Some(destination_index) = show.sequence_index(target_sequence) else {
                return Ok(String::new());
            };
            let Some(position) = show.sequences[source_index].cue_index(number) else {
                return Ok(String::new());
            };
            let mut cue = show.sequences[source_index].cues[position].clone();
            cue.number = to;
            let insertion = show.sequences[destination_index].insertion_index(to);
            show.sequences[destination_index]
                .cues
                .insert(insertion, cue);
            show.sequences[destination_index].rebase();
            if source_index != destination_index {
                show.sequences[source_index].rebase();
            }
            Ok(format!(
                "Copied cue {label} to cue {target_label} in sequence {target_sequence}"
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    /// Retime one cue's trigger. This is a show edit with one undo step, and it
    /// never restarts a running cue: the change takes effect the next time the
    /// chain is evaluated.
    fn cue_trigger(
        &mut self,
        sequence_number: u16,
        number: f64,
        trigger: TriggerArg,
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
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        }
        let label = format_trigger(trigger.trigger, trigger.time);
        let message = self.mutate(|show| {
            if let Some(index) = show.sequence_index(sequence_number) {
                if let Some(cue) = show.sequences[index].cue_mut(number) {
                    apply_trigger(cue, trigger);
                }
            }
            Ok(format!(
                "Cue {} in sequence {sequence_number} starts on {label}",
                format_cue_number(number)
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    /// `Cue 2 Timing …`: list one attribute's timing, list the cue's table, or
    /// change an override. Timing never restarts a running cue: a run keeps the
    /// plan it started with, and the change applies the next time it starts.
    fn cue_timing(
        &mut self,
        sequence_number: u16,
        number: f64,
        spec: TimingSpec,
    ) -> Result<CommandOutcome, CommandError> {
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "`Sequence` lists the sequences",
            ));
        };
        let Some(index) = sequence.cue_index(number) else {
            return Err(CommandError::with_hint(
                format!(
                    "sequence {sequence_number} has no cue {}",
                    format_cue_number(number)
                ),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        };
        let Some(attribute) = spec.attribute else {
            return Ok(CommandOutcome::with_lines(
                self.cue_timing_table(sequence_number, index),
            ));
        };
        let canonical = self.resolve_cue_attribute(sequence_number, index, &attribute)?;
        if spec.fade.is_none() && spec.delay.is_none() && spec.curve.is_none() && !spec.off {
            let line = self.cue_timing_line(sequence_number, index, &canonical);
            return Ok(CommandOutcome::message(line));
        }
        for (label, value) in [("fade", spec.fade), ("delay", spec.delay)] {
            if let Some(value) = value {
                if !value.is_finite() || !(0.0..=MAX_TIME).contains(&value) {
                    return Err(CommandError::new(format!(
                        "{label} {value} is outside 0..={MAX_TIME} seconds"
                    )));
                }
            }
        }
        let message = if spec.off {
            format!("`{canonical}` follows the cue fade and delay again")
        } else {
            let mut parts = Vec::new();
            if let Some(fade) = spec.fade {
                parts.push(format!("fade {fade:.1}s"));
            }
            if let Some(delay) = spec.delay {
                parts.push(format!("delay {delay:.1}s"));
            }
            if let Some(curve) = spec.curve {
                parts.push(curve.label().to_string());
            }
            format!("`{canonical}` {}", parts.join(", "))
        };
        self.mutate(|show| {
            if let Some(index) = show.sequence_index(sequence_number) {
                if let Some(cue) = show.sequences[index].cue_mut(number) {
                    if spec.off {
                        cue.timing.remove(&canonical);
                    } else {
                        let entry = cue.timing.entry(canonical.clone()).or_default();
                        if let Some(fade) = spec.fade {
                            entry.fade = Some(fade);
                        }
                        if let Some(delay) = spec.delay {
                            entry.delay = Some(delay);
                        }
                        if let Some(curve) = spec.curve {
                            entry.curve = curve;
                        }
                        // An entry that overrides nothing is not kept, so the
                        // stored map lists exactly what differs from the cue.
                        if entry.fade.is_none()
                            && entry.delay.is_none()
                            && entry.curve == Curve::Linear
                        {
                            cue.timing.remove(&canonical);
                        }
                    }
                }
            }
            Ok(message.clone())
        })?;
        Ok(CommandOutcome::message(format!(
            "Cue {} in sequence {sequence_number}: {message}",
            format_cue_number(number)
        )))
    }

    fn cue_stagger(
        &mut self,
        sequence_number: u16,
        number: f64,
        stagger: f64,
    ) -> Result<CommandOutcome, CommandError> {
        if !stagger.is_finite() || !(0.0..=MAX_TIME).contains(&stagger) {
            return Err(CommandError::new(format!(
                "stagger {stagger} is outside 0..={MAX_TIME} seconds"
            )));
        }
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
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        }
        let label = format_cue_number(number);
        let message = self.mutate(|show| {
            if let Some(index) = show.sequence_index(sequence_number) {
                if let Some(cue) = show.sequences[index].cue_mut(number) {
                    cue.stagger = stagger;
                }
            }
            Ok(if stagger > 0.0 {
                format!(
                    "Cue {label} in sequence {sequence_number}: each fixture starts {stagger:.2}s after the one before"
                )
            } else {
                format!("Cue {label} in sequence {sequence_number}: no stagger")
            })
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn cue_timing_table(&self, sequence_number: u16, index: usize) -> Vec<String> {
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return vec![format!("Sequence {sequence_number} does not exist.")];
        };
        let Some(cue) = sequence.cues.get(index) else {
            return vec![format!(
                "sequence {sequence_number} has no cue at that position"
            )];
        };
        let mut lines = vec![format!(
            "Cue {} in sequence {sequence_number}: fade {:.1}s, delay {:.1}s{}",
            format_cue_number(cue.number),
            cue.fade,
            cue.delay,
            if cue.stagger > 0.0 {
                format!(", stagger {:.2}s", cue.stagger)
            } else {
                String::new()
            }
        )];
        if cue.timing.is_empty() {
            lines.push(
                "No per-attribute timing: every attribute follows the cue fade and delay."
                    .to_string(),
            );
            return lines;
        }
        lines.push(format!(
            "{:<16} {:>8} {:>8}  {}",
            "Attribute", "Fade", "Delay", "Curve"
        ));
        for (attribute, timing) in &cue.timing {
            lines.push(format!(
                "{:<16} {:>7.1}s {:>7.1}s  {}",
                truncate(attribute, 16),
                timing.fade.unwrap_or(cue.fade),
                timing.delay.unwrap_or(cue.delay),
                timing.curve.label()
            ));
        }
        lines
    }

    fn cue_timing_line(&self, sequence_number: u16, index: usize, attribute: &str) -> String {
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return format!("Sequence {sequence_number} does not exist.");
        };
        let Some(cue) = sequence.cues.get(index) else {
            return "The cue no longer exists.".to_string();
        };
        let label = format!(
            "Cue {} in sequence {sequence_number}",
            format_cue_number(cue.number)
        );
        match timing_for(cue, attribute) {
            None => format!("{label}: `{attribute}` follows the cue fade and delay"),
            Some(timing) => format!(
                "{label}: `{attribute}` fade {:.1}s, delay {:.1}s, {}",
                timing.fade.unwrap_or(cue.fade),
                timing.delay.unwrap_or(cue.delay),
                timing.curve.label()
            ),
        }
    }

    /// Store a timing key in the spelling the cue or a fixture mode uses, so
    /// `Timing colour …` and `Timing ColorAdd_R …` name the same lane. An
    /// attribute none of the cue's fixtures has is rejected, naming it.
    fn resolve_cue_attribute(
        &self,
        sequence_number: u16,
        index: usize,
        typed: &str,
    ) -> Result<String, CommandError> {
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::new(format!(
                "sequence {sequence_number} does not exist"
            )));
        };
        let Some(cue) = sequence.cues.get(index) else {
            return Err(CommandError::new("the cue no longer exists"));
        };
        let state = sequence.state_at(index);
        let mut names: Vec<&String> = cue
            .levels
            .fixtures()
            .flat_map(|(_, attributes)| attributes.keys())
            .collect();
        names.extend(cue.timing.keys());
        names.extend(
            state
                .fixtures()
                .flat_map(|(_, attributes)| attributes.keys()),
        );
        if let Some(name) = names
            .into_iter()
            .find(|name| name.eq_ignore_ascii_case(typed))
        {
            return Ok(name.clone());
        }

        let fixtures: Vec<u16> = if cue.levels.is_empty() {
            self.show
                .fixtures
                .iter()
                .map(|fixture| fixture.number)
                .collect()
        } else {
            cue.levels.fixtures().map(|(number, _)| number).collect()
        };
        let mut known: Vec<&str> = Vec::new();
        for number in fixtures {
            let Some(fixture) = self.show.fixture(number) else {
                continue;
            };
            let Ok((fixture_type, _)) = self.show.mode_for(fixture) else {
                continue;
            };
            if let Some(attribute) = fixture_type.attribute(typed) {
                return Ok(attribute.name.clone());
            }
            known.extend(
                fixture_type
                    .attributes
                    .iter()
                    .map(|attribute| attribute.name.as_str()),
            );
        }
        known.sort_unstable();
        known.dedup();
        Err(CommandError::with_hint(
            format!(
                "`{typed}` is not an attribute of the fixtures cue {} uses",
                format_cue_number(cue.number)
            ),
            if known.is_empty() {
                "patch a fixture first: `Sheet` lists what is patched".to_string()
            } else {
                format!("known attributes: {}", known.join(", "))
            },
        ))
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
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        }
        // The generic validation failure says the show would be invalid; this
        // says which event depends on the cue and how to remove it.
        for timecode in &self.show.timecodes {
            if let Some(event) = timecode.events.iter().find(|event| {
                event.sequence == sequence_number && (event.cue - number).abs() < 1e-9
            }) {
                return Err(CommandError::with_hint(
                    format!(
                        "cue {} is started by timecode {} at {}; remove the event first",
                        format_cue_number(number),
                        timecode.number,
                        event.at
                    ),
                    format!("`Timecode {} Remove {}`", timecode.number, event.at),
                ));
            }
        }
        let message = self.mutate(|show| {
            if let Some(index) = show.sequence_index(sequence_number) {
                let sequence = &mut show.sequences[index];
                if let Some(position) = sequence.cue_index(number) {
                    sequence.cues.remove(position);
                }
                sequence.rebase();
            }
            Ok(format!(
                "Deleted cue {} from sequence {sequence_number}",
                format_cue_number(number)
            ))
        })?;
        let previewed = self.desk.preview.is_some_and(|preview| {
            preview.sequence == sequence_number && (preview.cue - number).abs() < 1e-9
        });
        if previewed {
            self.desk.preview = None;
        }
        Ok(CommandOutcome::message(if previewed {
            format!("{message}; preview cleared")
        } else {
            message
        }))
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
                        "{:<6} {:<18} {:>6} {:>6} {:<13} {:<4} {:<4} {}",
                        "Cue", "Name", "Fade", "Delay", "Trigger", "Mib", "Blk", "Values"
                    ));
                    for cue in &sequence.cues {
                        lines.push(format!(
                            "{:<6} {:<18} {:>5.1}s {:>5.1}s {:<13} {:<4} {:<4} {}{}",
                            format_cue_number(cue.number),
                            truncate(&cue.name, 18),
                            cue.fade,
                            cue.delay,
                            format_trigger(cue.trigger, cue.trigger_time),
                            if cue.mib { "yes" } else { "-" },
                            if cue.blocked { "yes" } else { "-" },
                            cue.levels.len(),
                            if cue.levels.is_empty() {
                                " (empty)"
                            } else {
                                ""
                            }
                        ));
                        // Only overrides print, so a plain cue list stays one
                        // line per cue.
                        for (attribute, timing) in &cue.timing {
                            if timing.fade.is_none()
                                && timing.delay.is_none()
                                && timing.curve == Curve::Linear
                            {
                                continue;
                            }
                            lines.push(format!(
                                "  Timing  {:<16} fade {:.1}s  delay {:.1}s  {}",
                                truncate(attribute, 16),
                                timing.fade.unwrap_or(cue.fade),
                                timing.delay.unwrap_or(cue.delay),
                                timing.curve.label()
                            ));
                        }
                        if cue.stagger > 0.0 {
                            lines.push(format!("  Stagger {:.2}s between fixtures", cue.stagger));
                        }
                        for (attribute, level) in &cue.shield {
                            lines.push(format!(
                                "  Shield  {:<16} {}%",
                                truncate(attribute, 16),
                                (level * 100.0).round() as i64
                            ));
                        }
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
                    mode: ExecutorMode::Go,
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
        let Some(mode) = self.show.executor(number).map(|executor| executor.mode) else {
            return Err(CommandError::with_hint(
                format!("executor {number} does not exist"),
                "assign one first: `Assign Executor 1 Sequence 1`",
            ));
        };
        let percent = (value * 100.0).round() as i64;
        self.desk.set_executor_fader(number, value);
        if mode == ExecutorMode::Fader {
            // A hand on the fader crossfades the cue: at 0 it holds the state
            // before the cue, at 100 the state of the cue. An idle list starts
            // cue 1 so the fader has something to move.
            if self.playback.run(Target::Executor(number)).is_none() {
                self.start_first_cue(number);
            }
            self.playback.scrub(Target::Executor(number), value);
        }
        self.refresh_playback();
        Ok(CommandOutcome::message(format!(
            "Executor {number} fader at {percent}%"
        )))
    }

    fn set_executor_mode(
        &mut self,
        number: u16,
        mode: ExecutorMode,
    ) -> Result<CommandOutcome, CommandError> {
        if self.show.executor(number).is_none() {
            return Err(CommandError::with_hint(
                format!("executor {number} does not exist"),
                "assign one first: `Assign Executor 1 Sequence 1`",
            ));
        }
        let label = mode.label();
        let message = self.mutate(|show| {
            if let Some(executor) = show
                .executors
                .iter_mut()
                .find(|executor| executor.number == number)
            {
                executor.mode = mode;
            }
            Ok(format!("Executor {number} mode {label}"))
        })?;
        Ok(CommandOutcome::message(message))
    }

    /// Hold or release flash. A desk action, not an edit: no undo step, and it
    /// never changes the show.
    fn set_executor_flash(
        &mut self,
        number: u16,
        on: bool,
    ) -> Result<CommandOutcome, CommandError> {
        if self.show.executor(number).is_none() {
            return Err(CommandError::with_hint(
                format!("executor {number} does not exist"),
                "assign one first: `Assign Executor 1 Sequence 1`",
            ));
        }
        self.desk.set_executor_flash(number, on);
        // Flash on an unstarted list starts its first cue; releasing restores
        // the fader but leaves the cue where it is.
        if on && self.playback.run(Target::Executor(number)).is_none() {
            self.start_first_cue(number);
        }
        self.refresh_playback();
        Ok(CommandOutcome::message(format!(
            "Executor {number} flash {}",
            if on { "on" } else { "off" }
        )))
    }

    /// Hold or release solo. Solo is a merge-time flag, so it alters no run.
    fn set_executor_solo(&mut self, number: u16, on: bool) -> Result<CommandOutcome, CommandError> {
        if self.show.executor(number).is_none() {
            return Err(CommandError::with_hint(
                format!("executor {number} does not exist"),
                "assign one first: `Assign Executor 1 Sequence 1`",
            ));
        }
        self.desk.set_executor_solo(number, on);
        self.refresh_playback();
        Ok(CommandOutcome::message(format!(
            "Executor {number} solo {}",
            if on { "on" } else { "off" }
        )))
    }

    /// Start the first cue of an executor's list, if it has one.
    fn start_first_cue(&mut self, executor: u16) {
        let Some(sequence) = self.show.executor(executor).map(|e| e.sequence) else {
            return;
        };
        if self
            .show
            .sequence(sequence)
            .map(|sequence| sequence.cues.is_empty())
            .unwrap_or(true)
        {
            return;
        }
        let now = self.clock;
        self.start_cue(Target::Executor(executor), sequence, 0, now);
    }

    /// `idle`, `flash`, `solo` or which cue is running.
    fn executor_state_label(&self, number: u16) -> String {
        let state = self.desk.executor_state(number);
        if state.flash {
            return "flash".to_string();
        }
        if state.solo {
            return "solo".to_string();
        }
        match self.playback.run(Target::Executor(number)) {
            Some(run) => format!(
                "cue {} {}",
                format_cue_number(run.cue_number),
                if run.is_paused() {
                    "held"
                } else if run.done {
                    "done"
                } else {
                    "running"
                }
            ),
            None => "idle".to_string(),
        }
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
            "{:<4} {:<12} {:<6} {:<7} {:<14} {}",
            "No.", "Sequence", "Mode", "Fader", "State", "Cue"
        )];
        for executor in executors {
            let cue = self
                .playback
                .run(Target::Executor(executor.number))
                .map(|run| format_cue_number(run.cue_number))
                .unwrap_or_else(|| "-".to_string());
            lines.push(format!(
                "{:<4} {:<12} {:<6} {:<7} {:<14} {}",
                executor.number,
                format!("Sequence {}", executor.sequence),
                executor.mode.label(),
                format!(
                    "{:.0}%",
                    self.desk.executor_state(executor.number).fader * 100.0
                ),
                self.executor_state_label(executor.number),
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
        self.playback.refresh(&self.desk, self.clock);
        self.revision += 1;
    }

    fn running_cue_label(&self, target: Target) -> String {
        match self.playback.run(target) {
            Some(run) => format!("cue {}", format_cue_number(run.cue_number)),
            None => "cue -".to_string(),
        }
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

        // Advance from the running cue's *number*, so deleting or inserting
        // cues under a running playback cannot make it jump over one.
        let index = match self.playback.run(target) {
            Some(run) => sequence.insertion_index(run.cue_number),
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
        let Some(cue_number) = self.playback.run(target).map(|run| run.cue_number) else {
            return Ok(CommandOutcome::message(format!(
                "{} is not running",
                target.label()
            )));
        };
        let Some(sequence) = self.show.sequence(sequence_number).cloned() else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "store a cue first: `Store Cue 1`",
            ));
        };
        // If the cue itself was deleted, go back from where it would sit.
        let position = sequence
            .cue_index(cue_number)
            .unwrap_or_else(|| sequence.insertion_index(cue_number));
        if position == 0 {
            return Ok(CommandOutcome::message(format!(
                "{} is at the first cue",
                target.label()
            )));
        }
        let index = position - 1;
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
        // Releasing hands the stage back to the programmer, so the temporary
        // full-intensity aid goes with it.
        let highlighted = std::mem::take(&mut self.desk.highlight);
        if self.playback.release(target) {
            self.refresh_playback();
            Ok(CommandOutcome::message(format!(
                "Released {}; output returns to the programmer",
                target.label()
            )))
        } else {
            if highlighted {
                self.revision += 1;
            }
            Ok(CommandOutcome::message(format!(
                "{} is not running",
                target.label()
            )))
        }
    }

    fn group_targets(&self, number: u16) -> Result<Vec<u16>, CommandError> {
        let Some(group) = self.show.group(number) else {
            return Err(CommandError::with_hint(
                format!("group {number} does not exist"),
                "store one first: select fixtures and run `Store Group 1`",
            ));
        };
        let fixtures: Vec<u16> = group
            .fixtures
            .iter()
            .copied()
            .filter(|fixture| self.show.fixture_index(*fixture).is_some())
            .collect();
        if fixtures.is_empty() {
            return Err(CommandError::with_hint(
                format!("group {number} has no patched fixtures"),
                "re-store it after patching the rig: select fixtures and `Store Group {number}`",
            ));
        }
        self.check_world(&fixtures)?;
        Ok(fixtures)
    }

    fn select_group(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        let fixtures = self.group_targets(number)?;
        self.select(fixtures)
    }

    fn set_level_group(
        &mut self,
        group: u16,
        attribute: Option<String>,
        source: LevelSource,
    ) -> Result<CommandOutcome, CommandError> {
        let fixtures = self.group_targets(group)?;
        self.set_level(Some(fixtures), attribute, source)
    }

    fn off_group(&mut self, group: u16) -> Result<CommandOutcome, CommandError> {
        self.set_level_group(group, None, LevelSource::Level(Level::Percent(0.0)))
    }

    /// Resolve the fixtures a `Default` or `Home` command addresses: a group,
    /// an explicit list, or the current selection.
    fn home_targets(
        &self,
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
    ) -> Result<Vec<u16>, CommandError> {
        match group {
            Some(number) => self.group_targets(number),
            None => self.targets(fixtures),
        }
    }

    /// `Fixture 1 Default …`: the base look below the programmer and playback.
    fn set_home(
        &mut self,
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
        attribute: Option<String>,
        level: Option<Level>,
        off: bool,
    ) -> Result<CommandOutcome, CommandError> {
        let targets = self.home_targets(fixtures, group)?;
        let Some(attribute) = attribute else {
            return Ok(self.home_list(&targets));
        };
        let canonical = self.resolve_attribute(&targets, &attribute)?;
        for number in &targets {
            let fixture = self
                .show
                .fixture(*number)
                .ok_or_else(|| CommandError::new(format!("fixture {number} is not patched")))?;
            let (_, mode) = self.show.mode_for(fixture).map_err(CommandError::new)?;
            if mode.channel_for(&canonical).is_none() {
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
        if off {
            let message = self.mutate(|show| {
                for number in &targets {
                    if let Some(index) = show.fixture_index(*number) {
                        show.fixtures[index].home.remove(&canonical);
                    }
                }
                Ok(format!("{label} `{canonical}` home: profile default again"))
            })?;
            return Ok(CommandOutcome::message(message));
        }

        let value = level
            .as_ref()
            .ok_or_else(|| CommandError::new("`Default` needs a level or `Off`"))
            .and_then(level_value)?;
        let percent = (value * 100.0).round() as i64;
        let message = self.mutate(|show| {
            for number in &targets {
                if let Some(index) = show.fixture_index(*number) {
                    show.fixtures[index].home.insert(canonical.clone(), value);
                }
            }
            Ok(format!("{label} `{canonical}` home at {percent}%"))
        })?;
        Ok(CommandOutcome::message(message))
    }

    /// The effective home for every attribute the fixtures have, marking which
    /// values are stored overrides and which come from the profile.
    fn home_list(&self, targets: &[u16]) -> CommandOutcome {
        let mut lines = Vec::new();
        for number in targets {
            let Some(fixture) = self.show.fixture(*number) else {
                continue;
            };
            let Ok((_, mode)) = self.show.mode_for(fixture) else {
                lines.push(format!("Fixture {number}: type or mode is not in the show"));
                continue;
            };
            lines.push(format!("Fixture {number} \"{}\"", fixture.name));
            let mut seen = BTreeSet::new();
            for channel in &mode.channels {
                if !seen.insert(channel.attribute.to_ascii_lowercase()) {
                    continue;
                }
                let stored = fixture
                    .home
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case(&channel.attribute));
                let level = self.show.home_level(fixture, &channel.attribute);
                lines.push(format!(
                    "  {:<16} {:>4}%  {}",
                    channel.attribute,
                    (level * 100.0).round() as i64,
                    if stored { "stored" } else { "profile" }
                ));
            }
            // A mode change keeps stored values the new mode lacks; say so
            // rather than hiding them.
            let mut extra: Vec<(&String, &f64)> = fixture
                .home
                .iter()
                .filter(|(name, _)| !seen.contains(&name.to_ascii_lowercase()))
                .collect();
            extra.sort_by_key(|(name, _)| name.as_str());
            for (name, level) in extra {
                lines.push(format!(
                    "  {:<16} {:>4}%  stored (not in mode)",
                    name,
                    (*level * 100.0).round() as i64
                ));
            }
        }
        CommandOutcome::with_lines(lines)
    }

    /// `Home`: put the base look into the programmer, where it can be stored or
    /// adjusted like any other programming.
    fn home(
        &mut self,
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
    ) -> Result<CommandOutcome, CommandError> {
        let targets = self.home_targets(fixtures, group)?;
        let mut values: Vec<(u16, String, f64)> = Vec::new();
        for number in &targets {
            let Some(fixture) = self.show.fixture(*number) else {
                continue;
            };
            let (_, mode) = self.show.mode_for(fixture).map_err(CommandError::new)?;
            let mut seen = BTreeSet::new();
            for channel in &mode.channels {
                if !seen.insert(channel.attribute.to_ascii_lowercase()) {
                    continue;
                }
                values.push((
                    *number,
                    channel.attribute.clone(),
                    self.show.home_level(fixture, &channel.attribute),
                ));
            }
        }
        let label = format_fixture_list(&targets);
        let count = values.len();
        let message = self.edit_programmer(move |programmer| {
            for (number, attribute, level) in &values {
                programmer.set(*number, attribute, *level);
            }
            format!("{label} recalled home ({count} values)")
        });
        self.desk.selection = targets;
        Ok(CommandOutcome::message(message))
    }

    fn store_group(
        &mut self,
        number: u16,
        name: Option<String>,
    ) -> Result<CommandOutcome, CommandError> {
        if number == 0 {
            return Err(CommandError::new("group number 0 is not allowed"));
        }
        let fixtures = self.programmer_targets()?;
        let mut created = false;
        let message = self.mutate(|show| {
            match show.groups.iter().position(|group| group.number == number) {
                Some(index) => {
                    show.groups[index].fixtures = fixtures.clone();
                    if let Some(name) = &name {
                        show.groups[index].name = name.clone();
                    }
                }
                None => {
                    created = true;
                    let mut group = Group::new(number);
                    group.fixtures = fixtures.clone();
                    if let Some(name) = &name {
                        group.name = name.clone();
                    }
                    show.groups.push(group);
                }
            }
            show.groups.sort_by_key(|group| group.number);
            Ok(format!(
                "{} group {number} ({})",
                if created { "Stored" } else { "Updated" },
                format_fixture_list(&fixtures)
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn delete_group(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        if self.show.group(number).is_none() {
            return Err(CommandError::with_hint(
                format!("group {number} does not exist"),
                "`Group` lists the stored groups",
            ));
        }
        let message = self.mutate(|show| {
            show.groups.retain(|group| group.number != number);
            Ok(format!("Deleted group {number}"))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn group_list(&self) -> CommandOutcome {
        if self.show.groups.is_empty() {
            return CommandOutcome::message(
                "No groups stored. Select fixtures and run `Store Group 1 \"Front\"`.",
            );
        }
        let mut lines = vec![format!(
            "{:<4} {:<20} {:<24} {}",
            "No.", "Name", "Fixtures", "Master"
        )];
        for group in &self.show.groups {
            lines.push(format!(
                "{:<4} {:<20} {:<24} {}%",
                group.number,
                truncate(
                    if group.name.trim().is_empty() {
                        "-"
                    } else {
                        &group.name
                    },
                    20
                ),
                format_fixture_list(&group.fixtures),
                (self.desk.masters.group(group.number) * 100.0).round() as i64
            ));
        }
        CommandOutcome::with_lines(lines)
    }

    fn store_world(
        &mut self,
        number: u16,
        name: Option<String>,
    ) -> Result<CommandOutcome, CommandError> {
        if number == 0 {
            return Err(CommandError::new("world number 0 is not allowed"));
        }
        let fixtures = self.programmer_targets()?;
        let mut created = false;
        let message = self.mutate(|show| {
            match show.worlds.iter().position(|world| world.number == number) {
                Some(index) => {
                    show.worlds[index].fixtures = fixtures.clone();
                    if let Some(name) = &name {
                        show.worlds[index].name = name.clone();
                    }
                }
                None => {
                    created = true;
                    let mut world = World::new(number);
                    world.fixtures = fixtures.clone();
                    if let Some(name) = &name {
                        world.name = name.clone();
                    }
                    show.worlds.push(world);
                }
            }
            show.worlds.sort_by_key(|world| world.number);
            Ok(format!(
                "{} world {number} ({})",
                if created { "Stored" } else { "Updated" },
                format_fixture_list(&fixtures)
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn delete_world(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        if self.show.world(number).is_none() {
            return Err(CommandError::with_hint(
                format!("world {number} does not exist"),
                "`World` lists the stored worlds",
            ));
        }
        let was_active = self.desk.world == Some(number);
        let message = self.mutate(|show| {
            show.worlds.retain(|world| world.number != number);
            Ok(format!("Deleted world {number}"))
        })?;
        if was_active {
            self.desk.world = None;
        }
        Ok(CommandOutcome::message(message))
    }

    fn activate_world(&mut self, number: Option<u16>) -> Result<CommandOutcome, CommandError> {
        let Some(number) = number else {
            self.desk.world = None;
            return Ok(CommandOutcome::message(
                "World off; every fixture is reachable",
            ));
        };
        let Some(world) = self.show.world(number) else {
            return Err(CommandError::with_hint(
                format!("world {number} does not exist"),
                "`World` lists the stored worlds",
            ));
        };
        self.desk.world = Some(number);
        Ok(CommandOutcome::message(format!(
            "Activated {} ({} fixture{})",
            world.display_name(),
            world.fixtures.len(),
            if world.fixtures.len() == 1 { "" } else { "s" }
        )))
    }

    fn world_list(&self) -> CommandOutcome {
        if self.show.worlds.is_empty() {
            return CommandOutcome::message(
                "No worlds stored. Select fixtures and run `Store World 1 \"Front\"`.",
            );
        }
        let mut lines = vec![format!(
            "{:<4} {:<20} {:<24} {}",
            "No.", "Name", "Fixtures", "Active"
        )];
        for world in &self.show.worlds {
            lines.push(format!(
                "{:<4} {:<20} {:<24} {}",
                world.number,
                truncate(
                    if world.name.trim().is_empty() {
                        "-"
                    } else {
                        &world.name
                    },
                    20
                ),
                format_fixture_list(&world.fixtures),
                if self.desk.world == Some(world.number) {
                    "yes"
                } else {
                    ""
                }
            ));
        }
        CommandOutcome::with_lines(lines)
    }

    fn known_pools(&self) -> BTreeSet<String> {
        let mut pools = BTreeSet::new();
        for fixture_type in &self.show.fixture_types {
            for mode in &fixture_type.modes {
                for channel in &mode.channels {
                    pools.insert(palette::pool_for(fixture_type, &channel.attribute));
                }
            }
        }
        pools
    }

    fn store_filter(
        &mut self,
        number: u16,
        name: Option<String>,
        features: Vec<String>,
    ) -> Result<CommandOutcome, CommandError> {
        if number == 0 {
            return Err(CommandError::new("filter number 0 is not allowed"));
        }
        let pools = self.known_pools();
        if pools.is_empty() {
            return Err(CommandError::with_hint(
                "no fixture types loaded, so there are no features to filter",
                "import one first: `Import Gdtf \"file.gdtf\"`",
            ));
        }
        // Store the pool's own spelling, so the filter reads back the same way
        // the palette pools do however the operator typed it.
        let mut allowed: Vec<String> = Vec::with_capacity(features.len());
        for feature in &features {
            let Some(pool) = pools
                .iter()
                .find(|pool| pool.eq_ignore_ascii_case(feature.trim()))
            else {
                return Err(CommandError::with_hint(
                    format!("unknown feature pool `{feature}`"),
                    format!(
                        "available pools: {}",
                        pools.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                ));
            };
            if allowed.iter().any(|existing| existing == pool) {
                return Err(CommandError::new(format!(
                    "feature `{pool}` is listed twice"
                )));
            }
            allowed.push(pool.clone());
        }
        let mut created = false;
        let label = allowed.join(", ");
        let message = self.mutate(|show| {
            match show
                .filters
                .iter()
                .position(|filter| filter.number == number)
            {
                Some(index) => {
                    show.filters[index].features = allowed.clone();
                    if let Some(name) = &name {
                        show.filters[index].name = name.clone();
                    }
                }
                None => {
                    created = true;
                    let mut filter = Filter::new(number);
                    filter.features = allowed.clone();
                    if let Some(name) = &name {
                        filter.name = name.clone();
                    }
                    show.filters.push(filter);
                }
            }
            show.filters.sort_by_key(|filter| filter.number);
            Ok(format!(
                "{} filter {number} ({label})",
                if created { "Stored" } else { "Updated" }
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn delete_filter(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        if self.show.filter(number).is_none() {
            return Err(CommandError::with_hint(
                format!("filter {number} does not exist"),
                "`Filter` lists the stored filters",
            ));
        }
        let was_active = self.desk.filter == Some(number);
        let message = self.mutate(|show| {
            show.filters.retain(|filter| filter.number != number);
            Ok(format!("Deleted filter {number}"))
        })?;
        if was_active {
            self.desk.filter = None;
        }
        Ok(CommandOutcome::message(message))
    }

    fn activate_filter(&mut self, number: Option<u16>) -> Result<CommandOutcome, CommandError> {
        let Some(number) = number else {
            self.desk.filter = None;
            return Ok(CommandOutcome::message(
                "Filter off; every feature can be programmed",
            ));
        };
        let Some(filter) = self.show.filter(number) else {
            return Err(CommandError::with_hint(
                format!("filter {number} does not exist"),
                "`Filter` lists the stored filters",
            ));
        };
        self.desk.filter = Some(number);
        Ok(CommandOutcome::message(format!(
            "Activated {} ({})",
            filter.display_name(),
            filter.features.join(", ")
        )))
    }

    fn filter_list(&self) -> CommandOutcome {
        if self.show.filters.is_empty() {
            return CommandOutcome::message(
                "No filters stored. Run `Store Filter 1 \"Intensity\" Dimmer`.",
            );
        }
        let mut lines = vec![format!(
            "{:<4} {:<20} {:<32} {}",
            "No.", "Name", "Features", "Active"
        )];
        for filter in &self.show.filters {
            lines.push(format!(
                "{:<4} {:<20} {:<32} {}",
                filter.number,
                truncate(
                    if filter.name.trim().is_empty() {
                        "-"
                    } else {
                        &filter.name
                    },
                    20
                ),
                truncate(&filter.features.join(", "), 32),
                if self.desk.filter == Some(filter.number) {
                    "yes"
                } else {
                    ""
                }
            ));
        }
        CommandOutcome::with_lines(lines)
    }

    fn set_blackout(
        &mut self,
        off: bool,
        fade: Option<f64>,
    ) -> Result<CommandOutcome, CommandError> {
        let fade = fade.unwrap_or(0.0);
        if !fade.is_finite() || !(0.0..=MAX_TIME).contains(&fade) {
            return Err(CommandError::new(format!(
                "blackout fade {fade} is outside 0..={MAX_TIME} seconds"
            )));
        }
        let level = if off { 1.0 } else { 0.0 };
        if !off {
            // Safety beats convenience: a blackout that a highlight can undo
            // is not a blackout.
            self.desk.highlight = false;
        }
        self.desk
            .set_blackout(level, std::time::Duration::from_secs_f64(fade), self.clock);
        self.revision += 1;
        Ok(CommandOutcome::message(if off {
            "Blackout lifted".to_string()
        } else {
            "Blackout".to_string()
        }))
    }

    /// `Blind` / `Blind Off`: keep the programmer off the stage without making
    /// a second buffer. The edit, the store and the listing all stay ordinary.
    fn set_blind(&mut self, on: bool) -> Result<CommandOutcome, CommandError> {
        self.desk.blind = on;
        self.revision += 1;
        Ok(CommandOutcome::message(if on {
            "Blind; the programmer no longer reaches the stage"
        } else {
            "Blind off; the programmer is live"
        }))
    }

    /// `Highlight` / `Highlight Off`: a render-stage output aid on the selected
    /// fixtures. It is never written to the programmer, so no store captures it.
    fn set_highlight(&mut self, on: bool) -> Result<CommandOutcome, CommandError> {
        if on && self.desk.selection.is_empty() {
            return Err(CommandError::with_hint(
                "nothing selected to highlight",
                "select fixtures first: `Fixture 1`",
            ));
        }
        self.desk.highlight = on;
        self.revision += 1;
        Ok(CommandOutcome::message(if on {
            "Highlight on; selected fixtures are at full intensity"
        } else {
            "Highlight off"
        }))
    }

    /// `Preview [Cue n] [Sequence s]` / `Preview Off`.
    fn preview(
        &mut self,
        cue: Option<f64>,
        sequence: Option<u16>,
        off: bool,
    ) -> Result<CommandOutcome, CommandError> {
        if off {
            return Ok(CommandOutcome::message(
                if self.desk.preview.take().is_some() {
                    "Preview off"
                } else {
                    "No preview is active"
                },
            ));
        }
        let sequence_number = self.preview_sequence_number(sequence)?;
        let sequence = self
            .show
            .sequence(sequence_number)
            .expect("preview_sequence_number checked the sequence");
        let number = match cue {
            Some(number) => number,
            None => {
                // The next cue after whatever this sequence is running; with
                // nothing running, its first cue.
                let current = self
                    .playback
                    .runs()
                    .filter(|(target, _)| {
                        self.sequence_number_for(*target).ok() == Some(sequence_number)
                    })
                    .map(|(_, run)| run.cue_number)
                    .next();
                let index = current.map_or(0, |number| sequence.insertion_index(number));
                sequence
                    .cues
                    .get(index)
                    .map(|cue| cue.number)
                    .ok_or_else(|| {
                        CommandError::with_hint(
                            format!("sequence {sequence_number} has no next cue"),
                            "name one: `Preview Cue <n>`",
                        )
                    })?
            }
        };
        let Some(index) = sequence.cue_index(number) else {
            return Err(CommandError::with_hint(
                format!(
                    "sequence {sequence_number} has no cue {}",
                    format_cue_number(number)
                ),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        };
        let values = sequence.state_at(index).len();
        let preview = Preview {
            sequence: sequence_number,
            cue: number,
        };
        self.desk.preview = Some(preview);
        Ok(CommandOutcome::message(format!(
            "Previewing {} ({values} value{}); `Preview Off` clears it",
            preview.label(),
            if values == 1 { "" } else { "s" }
        )))
    }

    /// Resolve which sequence a preview addresses: the one named, or the first
    /// running playback.
    fn preview_sequence_number(&self, requested: Option<u16>) -> Result<u16, CommandError> {
        if let Some(number) = requested {
            if self.show.sequence(number).is_none() {
                return Err(CommandError::with_hint(
                    format!("sequence {number} does not exist"),
                    "`Sequence` lists the sequences",
                ));
            }
            return Ok(number);
        }
        for (target, _) in self.playback.runs() {
            if let Ok(number) = self.sequence_number_for(target) {
                return Ok(number);
            }
        }
        // With one sequence there is nothing to disambiguate, so a bare
        // `Preview Cue 14` is the common single-list case.
        if self.show.sequences.len() == 1 {
            return Ok(self.show.sequences[0].number);
        }
        Err(CommandError::with_hint(
            "no playback is running to preview",
            "name one: `Preview Cue 14 Sequence 2`, or store a cue first",
        ))
    }

    /// `Status`'s preview summary.
    fn preview_summary(&self) -> String {
        let Some(preview) = self.desk.preview else {
            return "off".to_string();
        };
        match self.show.sequence(preview.sequence).and_then(|sequence| {
            sequence
                .cue_index(preview.cue)
                .map(|index| (sequence, index))
        }) {
            Some((sequence, index)) => {
                let values = sequence.state_at(index).len();
                format!(
                    "{} ({values} value{})",
                    preview.label(),
                    if values == 1 { "" } else { "s" }
                )
            }
            None => format!("{} (missing)", preview.label()),
        }
    }

    /// One `Sheet` line: the previewed cue and the first non-zero slots.
    fn preview_banner(&self) -> Option<String> {
        let preview = self.desk.preview?;
        let Some(frame) = self.preview_frame() else {
            return Some(format!(
                "Preview {}: no longer in the show",
                preview.label()
            ));
        };
        let mut count = 0usize;
        let mut first = Vec::new();
        for (universe, slots) in frame.universes() {
            for (offset, value) in slots.iter().enumerate() {
                if *value == 0 {
                    continue;
                }
                count += 1;
                if first.len() < 8 {
                    first.push(format!("{universe}.{:03} {value}", offset + 1));
                }
            }
        }
        let more = if count > first.len() {
            format!(" (+{} more)", count - first.len())
        } else {
            String::new()
        };
        Some(if count == 0 {
            format!("Preview {}: every channel is zero", preview.label())
        } else {
            format!(
                "Preview {}: {count} non-zero channel{} — {}{more}",
                preview.label(),
                if count == 1 { "" } else { "s" },
                first.join(", ")
            )
        })
    }

    fn lock_desk(&mut self, code: Option<String>) -> Result<CommandOutcome, CommandError> {
        if self.desk.lock.is_some() {
            return Ok(CommandOutcome::message(
                "The desk is already locked; `Unlock <code>` releases it",
            ));
        }
        self.desk.lock = Some(code.unwrap_or_default());
        Ok(CommandOutcome::message(
            "Desk locked; mutating commands are refused until `Unlock <code>`",
        ))
    }

    fn unlock_desk(&mut self, code: Option<String>) -> Result<CommandOutcome, CommandError> {
        let Some(required) = self.desk.lock.clone() else {
            return Ok(CommandOutcome::message("The desk is not locked"));
        };
        if constant_time_eq(required.as_bytes(), code.unwrap_or_default().as_bytes()) {
            self.desk.lock = None;
            Ok(CommandOutcome::message("Desk unlocked"))
        } else {
            Err(CommandError::with_hint(
                "wrong unlock code",
                "`Unlock <code>` — the desk stays locked",
            ))
        }
    }

    fn set_grand_master(&mut self, level: Level) -> Result<CommandOutcome, CommandError> {
        let value = level_value(&level)?;
        self.desk.masters.grand = value;
        self.revision += 1;
        Ok(CommandOutcome::message(format!(
            "Grand master at {}%",
            (value * 100.0).round() as i64
        )))
    }

    fn set_rate_master(&mut self, level: Level) -> Result<CommandOutcome, CommandError> {
        let value = match level {
            Level::Full => 1.0,
            Level::Percent(percent) => {
                if !(0.0..=(MAX_RATE_MASTER * 100.0)).contains(&percent) {
                    return Err(CommandError::new(format!(
                        "rate {percent}% is outside 0..={}%",
                        MAX_RATE_MASTER * 100.0
                    )));
                }
                percent / 100.0
            }
        };
        self.desk.masters.rate = value;
        self.revision += 1;
        Ok(CommandOutcome::message(format!(
            "Rate master at {}%",
            (value * 100.0).round() as i64
        )))
    }

    fn set_group_master(
        &mut self,
        number: u16,
        level: Level,
    ) -> Result<CommandOutcome, CommandError> {
        if self.show.group(number).is_none() {
            return Err(CommandError::with_hint(
                format!("group {number} does not exist"),
                "`Group` lists the stored groups",
            ));
        }
        let value = level_value(&level)?;
        self.desk.masters.set_group(number, value);
        self.revision += 1;
        Ok(CommandOutcome::message(format!(
            "Group {number} master at {}%",
            (value * 100.0).round() as i64
        )))
    }

    fn master_list(&self) -> CommandOutcome {
        let mut lines = vec![
            format!(
                "Grand master: {}%",
                (self.desk.masters.grand * 100.0).round() as i64
            ),
            format!(
                "Rate master: {}%",
                (self.desk.masters.rate * 100.0).round() as i64
            ),
        ];
        if self.show.groups.is_empty() {
            lines.push("Group masters: none (no groups stored)".into());
        } else {
            for group in &self.show.groups {
                lines.push(format!(
                    "{} master: {}%",
                    group.display_name(),
                    (self.desk.masters.group(group.number) * 100.0).round() as i64
                ));
            }
        }
        CommandOutcome::with_lines(lines)
    }

    fn store_effect(&mut self, spec: EffectSpec) -> Result<CommandOutcome, CommandError> {
        let EffectSpec {
            number,
            name,
            attribute,
            waveform,
            rate,
            size,
            spread,
        } = spec;
        if number == 0 {
            return Err(CommandError::new("effect number 0 is not allowed"));
        }
        let waveform = match waveform {
            Some(name) => Waveform::parse(&name).ok_or_else(|| {
                CommandError::with_hint(
                    format!("unknown waveform `{name}`"),
                    format!("use {}", Waveform::names()),
                )
            })?,
            None => Waveform::default(),
        };
        let attribute = attribute.unwrap_or_else(|| DIMMER.to_string());
        if attribute.trim().is_empty() {
            return Err(CommandError::new("an effect needs an attribute"));
        }
        let rate_hz = rate.unwrap_or(1.0);
        if !(0.0..=MAX_RATE_HZ).contains(&rate_hz) {
            return Err(CommandError::new(format!(
                "rate {rate_hz} Hz is outside 0..={MAX_RATE_HZ}"
            )));
        }
        let size = size.unwrap_or(50.0) / 100.0;
        if !(0.0..=1.0).contains(&size) {
            return Err(CommandError::new(
                "size is a percentage from 0 to 100".to_string(),
            ));
        }
        let spread = spread.unwrap_or(0.0);
        if !(0.0..=MAX_SPREAD_DEGREES).contains(&spread) {
            return Err(CommandError::new(format!(
                "spread {spread} is outside 0..={MAX_SPREAD_DEGREES} degrees"
            )));
        }

        let mut created = false;
        let message = self.mutate(|show| {
            let effect = Effect {
                number,
                name: name.clone().unwrap_or_default(),
                attribute: attribute.clone(),
                waveform,
                rate_hz,
                size,
                spread,
            };
            match show
                .effects
                .iter()
                .position(|effect| effect.number == number)
            {
                Some(index) => show.effects[index] = effect,
                None => {
                    created = true;
                    show.effects.push(effect);
                }
            }
            show.effects.sort_by_key(|effect| effect.number);
            Ok(format!(
                "{} effect {number} ({attribute}, {}, {rate_hz} Hz, {}%, spread {spread}°)",
                if created { "Stored" } else { "Updated" },
                waveform.name(),
                (size * 100.0).round() as i64
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn delete_effect(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        if self.show.effect(number).is_none() {
            return Err(CommandError::with_hint(
                format!("effect {number} does not exist"),
                "`Effect` lists the stored effects",
            ));
        }
        let message = self.mutate(|show| {
            show.effects.retain(|effect| effect.number != number);
            Ok(format!("Deleted effect {number}"))
        })?;
        self.effects.stop(number);
        Ok(CommandOutcome::message(message))
    }

    fn effect_on(
        &mut self,
        number: u16,
        fixtures: Option<Vec<u16>>,
    ) -> Result<CommandOutcome, CommandError> {
        let Some(effect) = self.show.effect(number).cloned() else {
            return Err(CommandError::with_hint(
                format!("effect {number} does not exist"),
                "store one first: `Store Effect 1`",
            ));
        };
        let targets = self.targets(fixtures)?;
        self.check_attribute_allowed(&targets, &effect.attribute)?;
        for fixture_number in &targets {
            let Some(fixture) = self.show.fixture(*fixture_number) else {
                continue;
            };
            let (_, mode) = self.show.mode_for(fixture).map_err(CommandError::new)?;
            if mode.channel_for(&effect.attribute).is_none() {
                return Err(CommandError::with_hint(
                    format!(
                        "fixture {fixture_number} has no `{}` in mode `{}`",
                        effect.attribute, fixture.mode
                    ),
                    format!("attributes: {}", attribute_list(mode)),
                ));
            }
        }
        self.effects.start(number, targets.clone(), self.clock);
        self.revision += 1;
        Ok(CommandOutcome::message(format!(
            "{} on {} ({}, {}, {} Hz, {}%)",
            effect.display_name(),
            format_fixture_list(&targets),
            effect.attribute,
            effect.waveform.name(),
            effect.rate_hz * self.desk.masters.rate,
            (effect.size * 100.0).round() as i64
        )))
    }

    fn effect_off(&mut self, effect: Option<u16>) -> Result<CommandOutcome, CommandError> {
        match effect {
            Some(number) => {
                if self.effects.stop(number) {
                    self.revision += 1;
                    Ok(CommandOutcome::message(format!("Stopped effect {number}")))
                } else {
                    Ok(CommandOutcome::message(format!(
                        "Effect {number} is not running"
                    )))
                }
            }
            None => {
                if self.effects.stop_all() {
                    self.revision += 1;
                    Ok(CommandOutcome::message("Stopped all effects"))
                } else {
                    Ok(CommandOutcome::message("No effects are running"))
                }
            }
        }
    }

    fn effect_list(&self, only: Option<u16>) -> CommandOutcome {
        let effects: Vec<&Effect> = match only {
            Some(number) => self.show.effect(number).into_iter().collect(),
            None => self.show.effects.iter().collect(),
        };
        if effects.is_empty() {
            return CommandOutcome::message(match only {
                Some(number) => format!("Effect {number} does not exist."),
                None => "No effects stored. Run `Store Effect 1 \"Pulse\"`.".to_string(),
            });
        }
        let mut lines = vec![format!(
            "{:<4} {:<20} {:<18} {:<9} {:<8} {:<6} {}",
            "No.", "Name", "Attribute", "Waveform", "Rate", "Size", "Spread"
        )];
        for effect in effects {
            lines.push(format!(
                "{:<4} {:<20} {:<18} {:<9} {:<8} {:<6} {}°",
                effect.number,
                truncate(
                    if effect.name.trim().is_empty() {
                        "-"
                    } else {
                        &effect.name
                    },
                    20
                ),
                truncate(&effect.attribute, 18),
                effect.waveform.name(),
                format!("{} Hz", effect.rate_hz),
                format!("{}%", (effect.size * 100.0).round() as i64),
                effect.spread.round() as i64
            ));
        }
        for (number, fixtures) in self.effects.runs() {
            if only.is_some_and(|wanted| wanted != number) {
                continue;
            }
            lines.push(format!(
                "Running: effect {number} on {}",
                format_fixture_list(fixtures)
            ));
        }
        CommandOutcome::with_lines(lines)
    }

    fn targets(&self, fixtures: Option<Vec<u16>>) -> Result<Vec<u16>, CommandError> {
        let list = fixtures.unwrap_or_else(|| self.desk.selection.clone());
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
        self.check_world(&list)?;
        Ok(list)
    }

    /// The active world, if any.
    fn active_world(&self) -> Option<&World> {
        self.desk.world.and_then(|number| self.show.world(number))
    }

    /// Whether the active world, if any, allows a fixture.
    fn world_allows(&self, fixture: u16) -> bool {
        match self.active_world() {
            Some(world) => world.allows(fixture),
            None => true,
        }
    }

    /// Refuse a target set that reaches outside the active world.
    fn check_world(&self, fixtures: &[u16]) -> Result<(), CommandError> {
        let Some(world) = self.active_world() else {
            return Ok(());
        };
        let blocked: Vec<u16> = fixtures
            .iter()
            .copied()
            .filter(|number| !world.allows(*number))
            .collect();
        if blocked.is_empty() {
            return Ok(());
        }
        Err(CommandError::with_hint(
            format!(
                "fixture {} is outside {}",
                format_fixture_list(&blocked),
                world.display_name()
            ),
            "select fixtures inside the world, or deactivate it with `World All`",
        ))
    }

    fn active_filter(&self) -> Option<&Filter> {
        self.desk.filter.and_then(|number| self.show.filter(number))
    }

    /// Whether the active filter, if any, allows programming an attribute.
    fn attribute_allowed(&self, fixture_type: &FixtureType, attribute: &str) -> bool {
        match self.active_filter() {
            Some(filter) => filter.allows(&palette::pool_for(fixture_type, attribute)),
            None => true,
        }
    }

    fn check_attribute_allowed(
        &self,
        targets: &[u16],
        attribute: &str,
    ) -> Result<(), CommandError> {
        let Some(filter) = self.active_filter() else {
            return Ok(());
        };
        for number in targets {
            let Some(fixture) = self.show.fixture(*number) else {
                continue;
            };
            let (fixture_type, _) = self.show.mode_for(fixture).map_err(CommandError::new)?;
            if !self.attribute_allowed(fixture_type, attribute) {
                return Err(CommandError::with_hint(
                    format!("`{attribute}` is outside {}", filter.display_name()),
                    format!("the filter allows: {}", filter.features.join(", ")),
                ));
            }
        }
        Ok(())
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

    /// Everything the timecode player and its active show are doing.
    pub fn timecode_status(&self) -> String {
        let Some(number) = self.timecode.active() else {
            return "idle".to_string();
        };
        let Some(timecode) = self.show.timecode(number) else {
            return "idle".to_string();
        };
        let position = self.timecode.position_at(timecode, self.clock);
        let state = if self.timecode.is_running() {
            "running"
        } else if self.timecode.is_paused() {
            "paused"
        } else {
            "stopped"
        };
        format!(
            "timecode {number} {state} {} ({} fps)",
            timecode::format(position, timecode.frame_rate()),
            timecode.frame_rate()
        )
    }

    fn store_timecode(
        &mut self,
        number: u16,
        name: Option<String>,
        fps: Option<u8>,
    ) -> Result<CommandOutcome, CommandError> {
        if number == 0 {
            return Err(CommandError::new("timecode number 0 is not allowed"));
        }
        if let Some(fps) = fps {
            if !(MIN_FPS..=MAX_FPS).contains(&fps) {
                return Err(CommandError::new(format!(
                    "frame rate {fps} is outside {MIN_FPS}..={MAX_FPS}"
                )));
            }
        }
        if let Some(name) = &name {
            if name.trim().is_empty() {
                return Err(CommandError::new("timecode name is empty"));
            }
        }
        let existed = self.show.timecode(number).is_some();
        let previous_fps = self.show.timecode(number).map(TimecodeShow::frame_rate);
        let mut message = self.mutate(|show| {
            match show
                .timecodes
                .iter_mut()
                .find(|timecode| timecode.number == number)
            {
                Some(timecode) => {
                    if let Some(name) = &name {
                        timecode.name = name.clone();
                    }
                    if let Some(fps) = fps {
                        timecode.fps = fps;
                    }
                }
                None => {
                    let mut timecode = TimecodeShow::new(number);
                    if let Some(name) = &name {
                        timecode.name = name.clone();
                    }
                    if let Some(fps) = fps {
                        timecode.fps = fps;
                    }
                    show.timecodes.push(timecode);
                    show.timecodes.sort_by_key(|timecode| timecode.number);
                }
            }
            let timecode = show
                .timecode(number)
                .ok_or_else(|| CommandError::new(format!("timecode {number} does not exist")))?;
            Ok(format!(
                "{} {} ({} fps, {})",
                if existed { "Updated" } else { "Stored" },
                timecode.display_name(),
                timecode.frame_rate(),
                count(timecode.events.len(), "event")
            ))
        })?;

        // Frame counts mean different times when the rate changes, so a player
        // on this show stops instead of firing events at the wrong moment.
        let current_fps = self.show.timecode(number).map(TimecodeShow::frame_rate);
        if previous_fps != current_fps && self.timecode.active() == Some(number) {
            self.timecode.stop();
            message.push_str("; the player was stopped because the frame rate changed");
        }
        Ok(CommandOutcome::message(message))
    }

    fn delete_timecode(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        if self.show.timecode(number).is_none() {
            return Err(CommandError::with_hint(
                format!("timecode {number} does not exist"),
                "`Timecode` lists the timecode shows",
            ));
        }
        let message = self.mutate(|show| {
            show.timecodes.retain(|timecode| timecode.number != number);
            Ok(format!("Deleted timecode {number}"))
        })?;
        if self.timecode.active() == Some(number) {
            self.timecode.stop();
        }
        Ok(CommandOutcome::message(message))
    }

    fn add_timecode_event(
        &mut self,
        number: u16,
        at: String,
        sequence_number: u16,
        cue: f64,
    ) -> Result<CommandOutcome, CommandError> {
        let Some(timecode) = self.show.timecode(number).cloned() else {
            return Err(CommandError::with_hint(
                format!("timecode {number} does not exist"),
                "store one first: `Store Timecode 1 \"Song\"`",
            ));
        };
        let frames = timecode::parse(&at, timecode.frame_rate()).map_err(CommandError::new)?;
        let Some(sequence) = self.show.sequence(sequence_number) else {
            return Err(CommandError::with_hint(
                format!("sequence {sequence_number} does not exist"),
                "store a cue first: `Store Cue 1`",
            ));
        };
        if sequence.cue_index(cue).is_none() {
            return Err(CommandError::with_hint(
                format!(
                    "sequence {sequence_number} has no cue {}",
                    format_cue_number(cue)
                ),
                format!("`Sequence {sequence_number}` lists its cues"),
            ));
        }

        let message = self.mutate(|show| {
            let Some(timecode) = show
                .timecodes
                .iter_mut()
                .find(|timecode| timecode.number == number)
            else {
                return Err(CommandError::new(format!(
                    "timecode {number} does not exist"
                )));
            };
            let fps = timecode.frame_rate();
            if timecode
                .events
                .iter()
                .any(|event| timecode::parse(&event.at, fps).ok() == Some(frames))
            {
                return Err(CommandError::new(format!(
                    "timecode {number} already has an event at {at}"
                )));
            }
            timecode.events.push(TimecodeEvent {
                at: at.clone(),
                sequence: sequence_number,
                cue,
            });
            timecode
                .events
                .sort_by_key(|event| timecode::parse(&event.at, fps).unwrap_or(u64::MAX));
            Ok(format!(
                "Added cue {} at {at} to timecode {number} ({})",
                format_cue_number(cue),
                count(timecode.events.len(), "event")
            ))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn remove_timecode_event(
        &mut self,
        number: u16,
        at: String,
    ) -> Result<CommandOutcome, CommandError> {
        let Some(timecode) = self.show.timecode(number).cloned() else {
            return Err(CommandError::with_hint(
                format!("timecode {number} does not exist"),
                "`Timecode` lists the timecode shows",
            ));
        };
        let frames = timecode::parse(&at, timecode.frame_rate()).map_err(CommandError::new)?;
        let message = self.mutate(|show| {
            let Some(timecode) = show
                .timecodes
                .iter_mut()
                .find(|timecode| timecode.number == number)
            else {
                return Err(CommandError::new(format!(
                    "timecode {number} does not exist"
                )));
            };
            let fps = timecode.frame_rate();
            let before = timecode.events.len();
            timecode
                .events
                .retain(|event| timecode::parse(&event.at, fps).ok() != Some(frames));
            if timecode.events.len() == before {
                return Err(CommandError::new(format!(
                    "timecode {number} has no event at {at}"
                )));
            }
            Ok(format!("Removed the event at {at} from timecode {number}"))
        })?;
        Ok(CommandOutcome::message(message))
    }

    fn start_timecode(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        let Some(timecode) = self.show.timecode(number) else {
            return Err(CommandError::with_hint(
                format!("timecode {number} does not exist"),
                "store one first: `Store Timecode 1 \"Song\"`",
            ));
        };
        let resume = self.timecode.is_paused() && self.timecode.active() == Some(number);
        let from = if resume { self.timecode.position() } else { 0 };
        self.timecode.start(number, from, self.clock);
        self.revision += 1;
        Ok(CommandOutcome::message(format!(
            "{} {} at {}",
            if resume { "Resumed" } else { "Started" },
            timecode.display_name(),
            timecode::format(from, timecode.frame_rate())
        )))
    }

    fn pause_timecode(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        let Some(timecode) = self.show.timecode(number).cloned() else {
            return Err(CommandError::with_hint(
                format!("timecode {number} does not exist"),
                "`Timecode` lists the timecode shows",
            ));
        };
        if self.timecode.active() != Some(number) || !self.timecode.is_running() {
            return Ok(CommandOutcome::message(format!(
                "Timecode {number} is not running"
            )));
        }
        self.timecode.pause(&timecode, self.clock);
        self.revision += 1;
        Ok(CommandOutcome::message(format!(
            "Paused timecode {number} at {}",
            timecode::format(self.timecode.position(), timecode.frame_rate())
        )))
    }

    fn stop_timecode(&mut self, number: u16) -> Result<CommandOutcome, CommandError> {
        if self.show.timecode(number).is_none() {
            return Err(CommandError::with_hint(
                format!("timecode {number} does not exist"),
                "`Timecode` lists the timecode shows",
            ));
        }
        if self.timecode.active() != Some(number) {
            return Ok(CommandOutcome::message(format!(
                "Timecode {number} is not running"
            )));
        }
        self.timecode.stop();
        self.revision += 1;
        Ok(CommandOutcome::message(format!(
            "Stopped timecode {number} at 00:00:00:00"
        )))
    }

    fn locate_timecode(&mut self, number: u16, at: &str) -> Result<CommandOutcome, CommandError> {
        let Some(timecode) = self.show.timecode(number).cloned() else {
            return Err(CommandError::with_hint(
                format!("timecode {number} does not exist"),
                "store one first: `Store Timecode 1 \"Song\"`",
            ));
        };
        let frames = timecode::parse(at, timecode.frame_rate()).map_err(CommandError::new)?;
        self.timecode.locate(number, frames, self.clock);
        self.revision += 1;
        Ok(CommandOutcome::message(format!(
            "Timecode {number} located to {} — {}",
            timecode::format(frames, timecode.frame_rate()),
            self.timecode_status()
        )))
    }

    fn timecode_list(&self, only: Option<u16>) -> CommandOutcome {
        match only {
            Some(number) => {
                let Some(timecode) = self.show.timecode(number) else {
                    return CommandOutcome::message(format!("Timecode {number} does not exist."));
                };
                let mut lines = vec![format!(
                    "{} — {} fps, {} events",
                    timecode.display_name(),
                    timecode.frame_rate(),
                    timecode.events.len()
                )];
                if timecode.events.is_empty() {
                    lines.push(format!(
                        "No events. Add one with `Timecode {number} Add 00:00:00:00 Sequence 1 Cue 1`."
                    ));
                } else {
                    lines.push(format!("{:<14} {:>8} {:>6}", "Time", "Sequence", "Cue"));
                    for event in &timecode.events {
                        lines.push(format!(
                            "{:<14} {:>8} {:>6}",
                            event.at,
                            event.sequence,
                            format_cue_number(event.cue)
                        ));
                    }
                }
                if self.timecode.active() == Some(number) {
                    lines.push(format!("Player: {}", self.timecode_status()));
                }
                CommandOutcome::with_lines(lines)
            }
            None => {
                if self.show.timecodes.is_empty() {
                    return CommandOutcome::message(
                        "No timecode shows. Store one with `Store Timecode 1 \"Song\"`.",
                    );
                }
                let mut lines = vec![format!(
                    "{:<4} {:<20} {:>4} {:>7}",
                    "No.", "Name", "Fps", "Events"
                )];
                for timecode in &self.show.timecodes {
                    lines.push(format!(
                        "{:<4} {:<20} {:>4} {:>7}",
                        timecode.number,
                        truncate(&timecode.name, 20),
                        timecode.frame_rate(),
                        timecode.events.len()
                    ));
                }
                if self.timecode.active().is_some() {
                    lines.push(format!("Player: {}", self.timecode_status()));
                }
                CommandOutcome::with_lines(lines)
            }
        }
    }

    fn timecode_events(&mut self, now: Instant) -> Vec<TimecodeEvent> {
        if !self.timecode.is_running() {
            return Vec::new();
        }
        let Some(number) = self.timecode.active() else {
            return Vec::new();
        };
        let Some(timecode) = self.show.timecode(number) else {
            self.timecode.stop();
            return Vec::new();
        };
        // Borrowing the show and the player as separate fields keeps the
        // 60 Hz tick from cloning the event list on every frame.
        self.timecode.advance(timecode, now)
    }

    /// Start the cue a timecode event names. Events that point at a cue that has
    /// since been deleted are skipped rather than stopping the track.
    fn fire_timecode_event(&mut self, event: &TimecodeEvent, now: Instant) {
        let Some(index) = self
            .show
            .sequence(event.sequence)
            .and_then(|sequence| sequence.cue_index(event.cue))
        else {
            return;
        };
        self.start_cue(Target::Sequence(event.sequence), event.sequence, index, now);
    }

    /// Start one cue on one target.
    ///
    /// Timecode and a trigger chain both come through here, so a cue starts the
    /// same way whoever asked for it.
    fn start_cue(&mut self, target: Target, sequence: u16, index: usize, now: Instant) {
        let _ = self.playback.go(&self.show, target, sequence, index, now);
    }

    fn history_step(&mut self, undo: bool) -> Result<CommandOutcome, CommandError> {
        let mut current = Snapshot {
            show: std::mem::take(&mut self.show),
            programmer: std::mem::take(&mut self.desk.programmer),
        };
        let label = if undo {
            self.history.undo(&mut current)
        } else {
            self.history.redo(&mut current)
        };
        self.show = current.show;
        self.desk.programmer = current.programmer;
        let Some(label) = label else {
            return Ok(CommandOutcome::message(if undo {
                "Nothing to undo."
            } else {
                "Nothing to redo."
            }));
        };
        self.after_history_restore();
        Ok(CommandOutcome::message(format!(
            "{} `{}` ({} undo, {} redo)",
            if undo { "Undid" } else { "Redid" },
            if label.is_empty() { "change" } else { &label },
            self.history.undo_depth(),
            self.history.redo_depth()
        )))
    }

    /// Undo can remove or restore any show object, so running state that points
    /// at something which no longer exists is dropped here rather than left to
    /// fail at render time.
    fn after_history_restore(&mut self) {
        self.desk
            .selection
            .retain(|number| self.show.fixture(*number).is_some());
        if self
            .desk
            .world
            .and_then(|number| self.show.world(number))
            .is_none()
        {
            self.desk.world = None;
        }
        if self
            .desk
            .filter
            .and_then(|number| self.show.filter(number))
            .is_none()
        {
            self.desk.filter = None;
        }
        self.playback.prune(&self.show);
        let live: BTreeSet<u16> = self
            .show
            .effects
            .iter()
            .map(|effect| effect.number)
            .collect();
        let stale: Vec<u16> = self
            .effects
            .runs()
            .map(|(number, _)| number)
            .filter(|number| !live.contains(number))
            .collect();
        for number in stale {
            self.effects.stop(number);
        }
        self.playback.refresh(&self.desk, self.clock);
        self.revision += 1;
    }

    /// Apply a programmer edit and record it for undo.
    ///
    /// Programmer values are desk state: they change no show data, so this
    /// neither validates nor saves the document, but the edit is still one
    /// undo step because "put that back" usually means the values too.
    fn edit_programmer<F>(&mut self, change: F) -> String
    where
        F: FnOnce(&mut Levels) -> String,
    {
        let snapshot = Snapshot {
            show: self.show.clone(),
            programmer: self.desk.programmer.clone(),
        };
        let message = change(&mut self.desk.programmer);
        if !self.recovering {
            self.history.record(snapshot, self.label.clone());
        }
        self.revision += 1;
        self.note(crate::journal::EntryKind::Programmer);
        message
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
        let changed = backup != self.show;
        if changed && !self.recovering {
            let programmer = self.desk.programmer.clone();
            self.history.record(
                Snapshot {
                    show: backup,
                    programmer,
                },
                self.label.clone(),
            );
        }
        self.revision += 1;
        if changed {
            self.note(crate::journal::EntryKind::Show);
        }
        Ok(message)
    }
}

pub const HELP: &[&str] = &[
    "Fixture 1 Thru 5                       select fixtures",
    "Fixture 1 Thru 5 At 50                 set Dimmer (0 to 100)",
    "Fixture 1 At ColorAdd_R 40             set any attribute of the mode",
    "Fixture 1 Gobo 2 / Gobo \"Rotate CW\"     choose a channel set",
    "Fixture 1 Gobo / Group 1 Gobo          list the sets and the current one",
    "At Palette Color 3                     apply a stored palette",
    "At 50 / Full / Off                     apply to the current selection",
    "Clear | Clear Values | Clear All        selection / programmer values",
    "Patch Fixture 1 At 1.001 Type \"<fixture type>\" [Mode \"<mode>\"]",
    "Clone Fixture 1 At 1.101 [Count 8] [Step 4]   copy a fixture across the rig",
    "Patch Check / Patch Import \"rig.csv\"          check or replace the patch",
    "Report Patch|Cues|Palettes|Status|Show [<sequence>] \"file\"",
    "Unpatch Fixture 1 Thru 3",
    "Import Gdtf \"file.gdtf\"                load a GDTF fixture type",
    "New [Show] [\"name\"]                    start an empty show",
    "Store Palette Color 1 \"Warm\"           capture the programmer in a pool",
    "Delete Palette Color 1 / Palette        delete or list palettes",
    "Store Cue 1 [\"name\"] [Fade 2] [Delay 0.5] [Mib] [Sequence 2]",
    "Store Cue 2 Follow 1.5 / Time 0.5  start without a button",
    "Store Cue 4 Merge / Replace           merge into the cue, or hold only the programmer",
    "Update [Cue 2] [Sequence 1]          merge the programmer into a cue",
    "Cue 2 Block / Unblock                stop or resume tracking",
    "Cue 2 Shield Pan [Off] / Shield      assert an attribute, or list the shields",
    "Cue 2 Move 5 / Cue 2 Copy 5 [Sequence 2]",
    "Cue 2 Show                           show the cue's absolute state",
    "Cue 2 Trigger Go / Follow 1 / Time 2  retime a stored cue",
    "Cue 2 Timing ColorAdd_R Fade 0.5 [Delay 0] [Curve EaseOut]",
    "Cue 2 Timing ColorAdd_R / Timing / Timing ColorAdd_R Off",
    "Cue 2 Stagger 0.2                   start each fixture after the one before",
    "Fixture 1 Default [Dimmer 50] / Default Dimmer Off  base look per fixture",
    "Home / Fixture 1 Home / Group 1 Home   recall the base look into the programmer",
    "Delete Cue 1 [Sequence 2] / Sequence [2]  delete or list cues",
    "Undo / Redo                            step through show edits",
    "Store Timecode 1 \"Song\" [Fps 25]       create a timecode show",
    "Timecode 1 Add 00:00:10:00 Sequence 1 Cue 2",
    "Timecode 1 Start / Pause / Stop / At 00:00:10:00",
    "Timecode [1] / Delete Timecode 1       list or delete timecode shows",
    "Assign Executor 1 Sequence 1            bind a cue list to a fader",
    "Executor 1 At 75 | Full | Off           executor master fader",
    "Executor 1 Mode Go | Fader | Flash | Solo",
    "Executor 1 Flash [Off] | Solo [Off]     hold or release a button",
    "Go | Back | Pause | Release [Executor 1 | Sequence 1]",
    "Group 1 / Group 1 At 50 / Group 1 Full / Group 1 Off",
    "Store Group 1 \"Front\" / Delete Group 1 / Group",
    "World 1 / World All / Store World 1 \"Front\" / World",
    "Filter 1 / Filter All / Store Filter 1 \"Intensity\" Dimmer Color",
    "Master Grand At 75 / Master Rate At 200 / Master Group 1 At 50 / Master",
    "Store Effect 1 \"Pulse\" [Attribute Dimmer] [Waveform Sine] [Rate 1] [Size 50] [Spread 0]",
    "Effect 1 On [Fixture 1 Thru 4] / Effect 1 Off / Effect Off / Effect",
    "Blackout [Fade 2] / Blackout Off      darken the rig without losing the look",
    "Blind / Blind Off                      edit without changing the stage",
    "Preview [Cue 14] [Sequence 2] / Preview Off  inspect a cue without sending it",
    "Highlight / Highlight Off              full intensity on selected fixtures",
    "Lock [code] / Unlock [code]           refuse mutating commands while locked",
    "Sheet                                  print the fixture sheet",
    "Status                                 output, show and playback state",
    "Save \"show.json\"                     write the show and its desk beside it",
    "Save Show \"show.json\" / Save Desk \"desk.oldesk\"   write one of the two",
    "Load \"show.json\"                     read the show and its desk",
    "Load Show \"show.json\"                read the show only, keep this desk",
    "Help                                   this list",
    "Separate several commands with `;`.",
];

fn next_fixture_number(used: &mut BTreeSet<u16>) -> Result<u16, CommandError> {
    let mut candidate = 1u16;
    loop {
        if !used.contains(&candidate) {
            used.insert(candidate);
            return Ok(candidate);
        }
        candidate = candidate
            .checked_add(1)
            .ok_or_else(|| CommandError::new("no free fixture numbers left"))?;
    }
}

fn text_of(outcome: CommandOutcome) -> String {
    let mut text = outcome.lines.join("\n");
    text.push('\n');
    text
}

fn join_sections(sections: Vec<Vec<String>>) -> Vec<String> {
    let mut lines = Vec::new();
    for section in sections {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(section);
    }
    lines
}

/// Apply a parsed trigger to a cue. A `Go` keeps the stored time, so switching
/// back and forth does not lose the number.
fn apply_trigger(cue: &mut Cue, spec: TriggerArg) {
    cue.trigger = spec.trigger;
    if spec.trigger != Trigger::Go {
        cue.trigger_time = spec.time;
    }
}

pub fn level_value(level: &Level) -> Result<f64, CommandError> {
    match level {
        Level::Percent(percent) => {
            if !(0.0..=100.0).contains(percent) {
                Err(CommandError::new(format!(
                    "level {percent} is outside 0..100"
                )))
            } else {
                Ok(percent / 100.0)
            }
        }
        Level::Full => Ok(1.0),
    }
}

impl Engine {
    /// Record the statement being dispatched, when it changed persisted state.
    fn note(&mut self, kind: crate::journal::EntryKind) {
        if self.recovering {
            return;
        }
        if let Some(journal) = &mut self.journal {
            journal.note(kind, &self.label, self.revision);
        }
    }
}

/// Compare two secrets without an early exit on the first difference.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

/// `1 step`, `2 steps`: keeps reports grammatical without a crate.
fn count(number: usize, singular: &str) -> String {
    format!("{number} {singular}{}", if number == 1 { "" } else { "s" })
}

/// Compact fixture list: `1 Thru 3 + 7`.
fn format_fixture_list(fixtures: &[u16]) -> String {
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

/// Explain why a set selector did not match a channel, naming the set count or
/// the names that do exist.
fn channel_set_error(
    number: u16,
    attribute: &str,
    channel: &Channel,
    selector: SetSelector<'_>,
) -> CommandError {
    match selector {
        SetSelector::Index(index) => {
            let count = channel.functions.len();
            CommandError::with_hint(
                format!(
                    "fixture {number} `{attribute}` has {count} set{}; {index} is out of range",
                    if count == 1 { "" } else { "s" }
                ),
                format!("`Fixture {number} {attribute}` lists them"),
            )
        }
        SetSelector::Name(name) => {
            let names: Vec<&str> = channel
                .sets()
                .map(|(_, function)| function.name.as_str())
                .collect();
            CommandError::with_hint(
                format!("fixture {number} `{attribute}` has no set `{name}`"),
                format!("sets: {}", names.join(", ")),
            )
        }
    }
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
    use crate::test_support::{demo_fixture_type, demo_moving_show, demo_show};
    use std::time::Duration;

    fn engine() -> Engine {
        Engine::new(demo_show())
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("openlight-engine-{}-{name}", std::process::id()))
    }

    fn two_fixture_engine() -> Engine {
        let mut engine = engine();
        engine
            .execute("Patch Fixture 2 At 1.005")
            .expect("demo type covers 4 channels at 1.001");
        engine
    }

    fn dimmer(engine: &Engine, fixture: u16) -> f64 {
        engine.desk.programmer.get(fixture, "Dimmer").unwrap()
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
            engine.desk.programmer.is_empty(),
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
            engine.desk.programmer.is_empty(),
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
        assert!(engine.desk.selection.is_empty());
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
            home: Default::default(),
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
        assert_eq!(engine.desk.programmer.get(2, "ColorAdd_R"), Some(1.0));
        assert_eq!(engine.desk.programmer.get(2, "ColorAdd_B"), Some(0.0));
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
    fn sheet_shows_the_live_output() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At 40").unwrap();
        assert!(engine.execute("Sheet").unwrap().lines[1].contains("Dimmer 40%"));

        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start);
        let sheet = engine.execute("Sheet").unwrap();
        assert!(
            sheet.lines[1].contains("Dimmer 40%"),
            "playback drives the sheet after the programmer is cleared: {:?}",
            sheet.lines
        );
    }

    #[test]
    fn deleting_a_cue_keeps_a_running_playback_on_its_own_number() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At 60").unwrap();
        engine.execute("Store Cue 2").unwrap();
        engine.execute("Clear All").unwrap();

        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start);
        engine.execute("Delete Cue 1").unwrap();
        let status = engine.execute("Status").unwrap().lines.join("\n");
        assert!(status.contains("cue 1"), "{status}");

        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start);
        assert_eq!(
            engine.render().slot(1, 1),
            Some(153),
            "Go moves to cue 2, which is now the only cue"
        );
        let status = engine.execute("Status").unwrap().lines.join("\n");
        assert!(status.contains("cue 2"), "{status}");
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
    fn update_merges_only_touched_attributes_and_is_one_undo_step() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear Values").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R 50").unwrap();
        engine.execute("Store Cue 2").unwrap();
        engine.execute("Clear Values").unwrap();

        engine.execute("Fixture 1 At 80").unwrap();
        let outcome = engine.execute("Update Cue 2").unwrap();
        assert!(outcome.lines[0].contains("Updated cue 2"), "{outcome:?}");
        let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
        assert_eq!(cue.levels.get(1, "Dimmer"), Some(0.8));
        assert_eq!(
            cue.levels.get(1, "ColorAdd_R"),
            Some(0.5),
            "the untouched attribute keeps its tracked value"
        );

        engine.execute("Undo").unwrap();
        let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
        assert_eq!(cue.levels.get(1, "Dimmer"), None, "one undo step");
    }

    #[test]
    fn update_without_a_running_cue_errors_with_a_hint() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        let error = engine.execute("Update").unwrap_err();
        assert!(error.message.contains("not running"), "{error}");
        assert!(
            error
                .hint
                .as_deref()
                .unwrap_or_default()
                .contains("Update Cue 2"),
            "{error}"
        );
    }

    #[test]
    fn merge_keeps_other_values_and_replace_drops_them() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R 50").unwrap();
        engine.execute("Store Cue 2").unwrap();
        engine.execute("Clear Values").unwrap();

        engine.execute("Fixture 1 At 80").unwrap();
        let outcome = engine.execute("Store Cue 2").unwrap();
        assert!(outcome.lines[0].contains("merge"), "{outcome:?}");
        let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
        assert_eq!(cue.levels.get(1, "Dimmer"), Some(0.8));
        assert_eq!(cue.levels.get(1, "ColorAdd_R"), Some(0.5));

        engine.execute("Clear Values").unwrap();
        engine.execute("Fixture 1 At 90").unwrap();
        let outcome = engine.execute("Store Cue 2 Replace").unwrap();
        assert!(outcome.lines[0].contains("replace"), "{outcome:?}");
        let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
        assert_eq!(cue.levels.get(1, "Dimmer"), Some(0.9));
        assert_eq!(
            cue.levels.get(1, "ColorAdd_R"),
            None,
            "replace drops what the programmer no longer has"
        );
    }

    #[test]
    fn storing_into_a_blocked_cue_keeps_it_blocked_and_absolute() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 2").unwrap();
        engine.execute("Cue 2 Block").unwrap();
        {
            let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
            assert!(cue.blocked);
            assert_eq!(cue.levels.get(1, "Dimmer"), Some(1.0));
            assert_eq!(cue.levels.len(), 1, "the absolute state, not a delta");
        }

        engine.execute("Clear Values").unwrap();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 2").unwrap();
        let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
        assert!(cue.blocked);
        assert_eq!(cue.levels.get(1, "Dimmer"), Some(0.4));
        assert_eq!(cue.levels.len(), 1);
    }

    #[test]
    fn move_keeps_looks_and_refuses_a_taken_number() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 2").unwrap();

        let error = engine.execute("Cue 1 Move 2").unwrap_err();
        assert!(error.message.contains("already has cue 2"), "{error}");

        engine.execute("Cue 1 Move 5").unwrap();
        let sequence = engine.show.sequence(1).unwrap();
        assert!(sequence.cue(1.0).is_none());
        assert_eq!(
            sequence.cue(5.0).unwrap().levels.get(1, "Dimmer"),
            Some(0.2),
            "the moved cue still holds its own look"
        );
        assert_eq!(
            sequence.state_at(1).get(1, "Dimmer"),
            Some(0.2),
            "and its absolute state is unchanged"
        );
    }

    #[test]
    fn copy_carries_timing_and_a_shield_to_another_sequence() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At 80").unwrap();
        engine.execute("Store Cue 2 Fade 1.5").unwrap();
        engine.execute("Cue 2 Shield Dimmer").unwrap();
        engine.execute("Store Cue 1 Sequence 2").unwrap();

        let outcome = engine.execute("Cue 2 Copy 5 Sequence 2").unwrap();
        assert!(
            outcome.lines[0].contains("Copied cue 2 to cue 5"),
            "{outcome:?}"
        );
        let copy = engine.show.sequence(2).unwrap().cue(5.0).unwrap();
        assert_eq!(copy.fade, 1.5);
        assert_eq!(copy.shield.get("Dimmer"), Some(&0.8));
    }

    #[test]
    fn delete_cue_names_the_timecode_event_that_blocks_it() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Store Timecode 1").unwrap();
        engine
            .execute("Timecode 1 Add 00:00:10:00 Sequence 1 Cue 1")
            .unwrap();

        let error = engine.execute("Delete Cue 1").unwrap_err();
        assert!(
            error.message.contains("timecode 1 at 00:00:10:00"),
            "{error}"
        );
        assert!(
            error
                .hint
                .as_deref()
                .unwrap_or_default()
                .contains("Timecode 1 Remove 00:00:10:00"),
            "{error}"
        );
        assert!(engine.show.sequence(1).unwrap().cue(1.0).is_some());
    }

    #[test]
    fn a_blocked_cue_absorbs_a_later_upstream_edit() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 2").unwrap();
        engine.execute("Cue 2 Block").unwrap();
        engine.execute("Clear Values").unwrap();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Update Cue 1").unwrap();

        assert_eq!(
            engine
                .show
                .sequence(1)
                .unwrap()
                .state_at(1)
                .get(1, "Dimmer"),
            Some(1.0),
            "the blocked cue ignored the edit to cue 1"
        );
        assert_eq!(
            engine
                .show
                .sequence(1)
                .unwrap()
                .state_at(0)
                .get(1, "Dimmer"),
            Some(0.4)
        );
    }

    #[test]
    fn a_shield_lists_and_holds_until_removed() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R 50").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear Values").unwrap();
        engine.execute("Fixture 1 At 80").unwrap();
        engine.execute("Store Cue 2").unwrap();
        engine.execute("Cue 2 Shield ColorAdd_R").unwrap();

        let listing = engine.execute("Cue 2 Shield").unwrap().lines.join("\n");
        assert!(
            listing.contains("ColorAdd_R") && listing.contains("50%"),
            "{listing}"
        );

        engine.execute("Clear Values").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R Full").unwrap();
        engine.execute("Update Cue 1").unwrap();
        let state = engine.show.sequence(1).unwrap().state_at(1);
        assert_eq!(state.get(1, "ColorAdd_R"), Some(0.5), "the shield holds");
        assert_eq!(state.get(1, "Dimmer"), Some(0.8), "other values track");

        engine.execute("Cue 2 Shield ColorAdd_R Off").unwrap();
        assert_eq!(
            engine
                .show
                .sequence(1)
                .unwrap()
                .state_at(1)
                .get(1, "ColorAdd_R"),
            Some(1.0),
            "removing the shield lets the upstream value through"
        );
    }

    #[test]
    fn cue_show_prints_the_absolute_state_and_the_listing_marks_blocks() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At 80").unwrap();
        engine.execute("Store Cue 2").unwrap();
        engine.execute("Cue 2 Block").unwrap();

        let shown = engine.execute("Cue 2 Show").unwrap().lines.join("\n");
        assert!(shown.contains("absolute state"), "{shown}");
        assert!(shown.contains("blocked"), "{shown}");
        assert!(shown.contains("Dimmer 80%"), "{shown}");

        let listed = engine.execute("Sequence 1").unwrap().lines.join("\n");
        assert!(listed.contains("Blk"), "{listed}");
        assert!(listed.contains("yes"), "{listed}");
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

        let error = engine.execute("Store Cue 2000000").unwrap_err();
        assert!(error.message.contains("outside"), "{error}");
    }

    #[test]
    fn loading_a_show_restores_its_desk_and_a_bare_show_starts_clean() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        assert!(engine.playback.is_running());

        let path = std::env::temp_dir().join(format!(
            "openlight-phase2-{}-load.olshow",
            std::process::id()
        ));
        engine
            .execute(&format!("Save \"{}\"", path.display()))
            .unwrap();
        // The desk recorded where the playback was, so reopening puts it back.
        engine
            .execute(&format!("Load \"{}\"", path.display()))
            .unwrap();
        assert!(engine.playback.is_running(), "the desk restored the run");
        assert_eq!(engine.render().slot(1, 1), Some(255));

        // A show copied without its desk starts clean rather than resuming a
        // run from another desk.
        let bare = path.with_file_name("openlight-bare.olshow");
        std::fs::copy(&path, &bare).unwrap();
        let _ = std::fs::remove_file(Desk::path_for(&bare));
        engine
            .execute(&format!("Load \"{}\"", bare.display()))
            .unwrap();
        assert!(!engine.playback.is_running());
        assert_eq!(engine.render().slot(1, 1), Some(0));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(Desk::path_for(&path));
        let _ = std::fs::remove_file(&bare);
    }

    #[test]
    fn the_programmer_is_not_saved_in_the_show_file() {
        let mut engine = engine();
        let path = std::env::temp_dir().join(format!(
            "openlight-programmer-{}.olshow",
            std::process::id()
        ));
        engine.execute("Fixture 1 At 50").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear Values").unwrap();
        engine.execute("Fixture 1 At 90").unwrap();
        engine
            .execute(&format!("Save \"{}\"", path.display()))
            .unwrap();

        engine.execute("New").unwrap();
        engine
            .execute(&format!("Load \"{}\"", path.display()))
            .unwrap();
        // The saved programmer came back with the desk...
        assert_eq!(dimmer(&engine, 1), 0.9);
        // ...but the show file itself never carried it.
        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        for fixture in value["fixtures"].as_array().unwrap() {
            assert!(
                fixture.get("levels").is_none(),
                "the document carries no programmer"
            );
        }
        assert!(
            Desk::path_for(&path).exists(),
            "the desk was written beside it"
        );
        let without_desk = std::env::temp_dir().join(format!(
            "openlight-programmer-bare-{}.olshow",
            std::process::id()
        ));
        std::fs::copy(&path, &without_desk).unwrap();
        let _ = std::fs::remove_file(Desk::path_for(&without_desk));
        engine.execute("Clear Values").unwrap();
        engine
            .execute(&format!("Load \"{}\"", without_desk.display()))
            .unwrap();
        assert!(engine.desk.programmer.is_empty());
        assert_eq!(
            engine.show.sequence(1).unwrap().cues[0]
                .levels
                .get(1, "Dimmer"),
            Some(0.5)
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(Desk::path_for(&path));
        let _ = std::fs::remove_file(&without_desk);
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
        assert!(engine.desk.selection.is_empty());
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

    #[test]
    fn a_group_is_selected_and_addressed_in_one_word() {
        let mut engine = two_fixture_engine();
        engine.execute("Fixture 1 Thru 2").unwrap();
        let stored = engine.execute("Store Group 1 \"All\"").unwrap();
        assert!(stored.lines[0].contains("1 + 2"), "{:?}", stored.lines);

        engine.execute("Clear").unwrap();
        engine.execute("Group 1 At 40").unwrap();
        assert_eq!(dimmer(&engine, 1), 0.4);
        assert_eq!(dimmer(&engine, 2), 0.4);
        assert_eq!(
            engine.desk.selection,
            vec![1, 2],
            "the group stays selected"
        );

        engine.execute("Group 1 Off").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(0));
        let list = engine.execute("Group").unwrap().lines.join("\n");
        assert!(list.contains("All"), "{list}");
    }

    #[test]
    fn group_and_grand_masters_scale_intensity_at_the_output() {
        let mut engine = two_fixture_engine();
        engine.execute("Fixture 1 Thru 2 At Full").unwrap();
        engine.execute("Store Group 1").unwrap();

        engine.execute("Master Group 1 At 50").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(128));
        assert_eq!(engine.render().slot(1, 5), Some(128));

        engine.execute("Master Grand At 50").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(64));

        engine.execute("Master Grand Off").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(0));
        assert_eq!(
            dimmer(&engine, 1),
            1.0,
            "masters never touch the programmer"
        );

        let masters = engine.execute("Master").unwrap().lines.join("\n");
        assert!(masters.contains("Grand master: 0%"), "{masters}");
        assert!(masters.contains("Group 1"), "{masters}");
    }

    #[test]
    fn a_world_limits_control_but_not_running_output() {
        let mut engine = two_fixture_engine();
        engine.execute("Fixture 1 Thru 2 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();

        engine.execute("Fixture 1").unwrap();
        engine.execute("Store World 1 \"Front\"").unwrap();
        engine.execute("World 1").unwrap();

        let error = engine.execute("Fixture 2 At 50").unwrap_err();
        assert!(error.message.contains("outside"), "{error}");
        assert!(error.hint.unwrap().contains("World All"));

        engine.execute("Go Sequence 1").unwrap();
        assert_eq!(
            engine.render().slot(1, 5),
            Some(255),
            "a cue still drives fixtures outside the world"
        );

        engine.execute("World All").unwrap();
        engine.execute("Fixture 2").unwrap();
    }

    #[test]
    fn a_filter_limits_programming_and_what_a_cue_captures() {
        let mut engine = engine();
        engine.execute("Fixture 1 At ColorAdd_R 100").unwrap();
        engine.execute("Store Palette Color 1").unwrap();
        engine.execute("Fixture 1 At 50").unwrap();

        engine
            .execute("Store Filter 1 \"Intensity\" dimmer")
            .unwrap();
        assert_eq!(
            engine.show.filter(1).unwrap().features,
            vec!["Dimmer".to_string()],
            "a feature is stored in the pool's own spelling"
        );
        engine.execute("Filter 1").unwrap();
        engine.execute("Fixture 1 At 60").unwrap();

        let error = engine.execute("Fixture 1 At ColorAdd_R 50").unwrap_err();
        assert!(error.message.contains("outside"), "{error}");
        let error = engine.execute("At Palette Color 1").unwrap_err();
        assert!(error.message.contains("outside"), "{error}");

        engine.execute("Store Cue 1").unwrap();
        let cue = &engine.show.sequence(1).unwrap().cues[0];
        assert_eq!(cue.levels.get(1, "Dimmer"), Some(0.6));
        assert_eq!(
            cue.levels.get(1, "ColorAdd_R"),
            None,
            "the filter kept colour out of the cue"
        );
    }

    #[test]
    fn effects_animate_the_output_between_ticks() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Effect 1").unwrap();
        engine.execute("Effect 1 On").unwrap();

        engine.tick(start + Duration::from_millis(250));
        assert_eq!(engine.render().slot(1, 1), Some(255), "full plus offset");
        engine.tick(start + Duration::from_millis(750));
        assert_eq!(engine.render().slot(1, 1), Some(128), "full minus offset");

        engine.execute("Effect 1 Off").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(255), "the base is intact");
    }

    #[test]
    fn the_rate_master_speeds_effects_up() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Effect 1").unwrap();
        engine.execute("Effect 1 On").unwrap();
        engine.execute("Master Rate At 200").unwrap();

        engine.tick(start + Duration::from_millis(125));
        assert_eq!(engine.render().slot(1, 1), Some(255));
        engine.tick(start + Duration::from_millis(375));
        assert_eq!(engine.render().slot(1, 1), Some(128));
    }

    #[test]
    fn effects_respect_worlds_and_filters() {
        let mut engine = two_fixture_engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Filter 1 Dimmer").unwrap();
        engine
            .execute("Store Effect 1 Attribute ColorAdd_R")
            .unwrap();
        engine.execute("Filter 1").unwrap();
        let error = engine.execute("Effect 1 On").unwrap_err();
        assert!(error.message.contains("outside"), "{error}");
        engine.execute("Filter All").unwrap();

        engine.execute("Store World 1").unwrap();
        engine.execute("World 1").unwrap();
        engine.execute("Store Effect 2").unwrap();
        let error = engine.execute("Effect 2 On Fixture 2").unwrap_err();
        assert!(error.message.contains("outside"), "{error}");
        engine.execute("Effect 2 On").unwrap();
    }

    #[test]
    fn deleting_an_effect_stops_it_and_removes_it_from_the_show() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Effect 1").unwrap();
        engine.execute("Effect 1 On").unwrap();
        assert!(engine
            .execute("Effect")
            .unwrap()
            .lines
            .join("\n")
            .contains("Running"));

        engine.execute("Delete Effect 1").unwrap();
        assert!(!engine.effects.is_running());
        assert!(engine.show.effects.is_empty());
        assert_eq!(engine.render().slot(1, 1), Some(255));
    }

    #[test]
    fn phase_three_objects_survive_a_save_and_load() {
        let mut engine = two_fixture_engine();
        let path = std::env::temp_dir().join(format!(
            "openlight-phase3-{}.olshow.json",
            std::process::id()
        ));
        engine.execute("Fixture 1 Thru 2 At Full").unwrap();
        engine.execute("Store Group 1 \"All\"").unwrap();
        engine.execute("Store World 2").unwrap();
        engine.execute("Store Filter 3 Dimmer").unwrap();
        engine.execute("Store Effect 4 \"Pulse\" Rate 2").unwrap();
        engine.execute("Master Grand At 40").unwrap();
        engine.execute("Master Rate At 200").unwrap();
        engine.execute("Master Group 1 At 30").unwrap();
        engine
            .execute(&format!("Save \"{}\"", path.display()))
            .unwrap();
        engine
            .execute(&format!("Load \"{}\"", path.display()))
            .unwrap();

        assert_eq!(engine.show.groups.len(), 1);
        assert_eq!(engine.desk.masters.group(1), 0.3);
        assert_eq!(engine.show.worlds.len(), 1);
        assert_eq!(engine.show.filters.len(), 1);
        assert_eq!(engine.show.effects.len(), 1);
        assert_eq!(engine.show.effects[0].rate_hz, 2.0);
        assert_eq!(engine.desk.masters.grand, 0.4);
        assert_eq!(engine.desk.masters.rate, 2.0);
        assert!(!engine.effects.is_running(), "runs are not saved");
        assert_eq!(engine.desk.world, None);
        assert_eq!(engine.desk.filter, None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn phase_three_commands_report_bad_input() {
        let mut engine = engine();
        assert!(engine.execute("Group 1").is_err(), "no groups yet");
        assert!(engine.execute("World 1").is_err());
        assert!(engine.execute("Filter 1").is_err());
        assert!(engine.execute("Fixture 1").is_ok());
        assert!(engine.execute("Store Filter 1 Colour").is_err());
        assert!(engine.execute("Master Grand At 101").is_err());
        assert!(engine.execute("Master Rate At 401").is_err());
        assert!(engine.execute("Master Group 1 At 50").is_err());
        assert!(engine.execute("Store Effect 1 Waveform Wobble").is_err());
        assert!(engine.execute("Store Effect 2 Size 150").is_err());
        assert!(engine.execute("Store Effect 3 Spread 400").is_err());
        assert!(engine.execute("Effect 9 On").is_err());
        assert!(engine.execute("Store Group 2").is_ok(), "selection exists");
        assert!(engine.execute("Group 2").is_ok());
        assert!(engine.execute("Delete Group 2").is_ok());
        assert!(engine.execute("Delete Group 2").is_err());
    }

    #[test]
    fn status_describes_pools_masters_and_effects() {
        let mut engine = engine();
        engine.execute("Fixture 1").unwrap();
        engine.execute("Store Group 1").unwrap();
        engine.execute("Store World 1").unwrap();
        engine.execute("Store Filter 1 Dimmer").unwrap();
        engine.execute("Store Effect 1").unwrap();
        engine.execute("Effect 1 On").unwrap();
        engine.execute("World 1").unwrap();
        engine.execute("Filter 1").unwrap();

        let status = engine.execute("Status").unwrap().lines.join("\n");
        assert!(status.contains("Groups: 1 | Worlds: 1"), "{status}");
        assert!(
            status.contains("Masters: grand 100% | rate 100%"),
            "{status}"
        );
        assert!(status.contains("World: World 1"), "{status}");
        assert!(status.contains("Filter: Filter 1"), "{status}");
        assert!(status.contains("Effect 1: running on 1"), "{status}");
        assert_eq!(
            engine.render().slot(1, 1),
            Some(0),
            "dimmer at 0 with no level"
        );
    }

    #[test]
    fn undo_and_redo_step_through_show_edits() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 1 Fade 2").unwrap();
        assert_eq!(engine.show.sequences.len(), 1);

        let undone = engine.execute("Undo").unwrap();
        assert!(undone.lines[0].starts_with("Undid"), "{undone:?}");
        assert!(undone.lines[0].contains("Store Cue 1 Fade 2"));
        assert!(engine.show.sequences.is_empty(), "the store was undone");
        assert_eq!(dimmer(&engine, 1), 0.4, "console state is not undone");

        let undone = engine.execute("Undo").unwrap();
        assert!(undone.lines[0].contains("Fixture 1 At 40"));
        assert!(engine.desk.programmer.is_empty());
        assert!(engine.execute("Undo").unwrap().lines[0].contains("Nothing to undo"));

        let redone = engine.execute("Redo").unwrap();
        assert!(redone.lines[0].starts_with("Redid"), "{redone:?}");
        assert_eq!(dimmer(&engine, 1), 0.4);
        engine.execute("Redo").unwrap();
        assert_eq!(engine.show.sequences.len(), 1);
        assert!(engine.execute("Redo").unwrap().lines[0].contains("Nothing to redo"));
    }

    #[test]
    fn a_new_edit_drops_the_redo_branch() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Undo").unwrap();
        engine.execute("Fixture 1 At 60").unwrap();
        let redo = engine.execute("Redo").unwrap();
        assert!(redo.lines[0].contains("Nothing to redo"), "{redo:?}");
        assert_eq!(dimmer(&engine, 1), 0.6);
    }

    #[test]
    fn undo_covers_show_edits_not_selection_or_rejected_commands() {
        let mut engine = engine();
        engine.execute("Fixture 1").unwrap();
        assert!(engine.execute("Undo").unwrap().lines[0].contains("Nothing to undo"));
        assert_eq!(engine.desk.selection, vec![1]);

        engine.execute("Fixture 1 At 30").unwrap();
        assert!(engine.execute("Fixture 1 At 101").is_err());
        engine.execute("Undo").unwrap();
        assert!(engine.desk.programmer.is_empty());
        assert!(
            engine.execute("Undo").unwrap().lines[0].contains("Nothing to undo"),
            "the rejected command left no step"
        );
    }

    #[test]
    fn move_in_black_pre_positions_a_dark_fixture() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At 0").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R 0").unwrap();
        engine.execute("Store Cue 1 Fade 0").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R Full").unwrap();
        engine.execute("Store Cue 2 Fade 1 Mib").unwrap();

        let listing = engine.execute("Sequence 1").unwrap().lines.join("\n");
        assert!(
            listing.contains("Mib") && listing.contains("yes"),
            "{listing}"
        );

        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start + Duration::from_millis(500));
        assert_eq!(
            engine.render().slot(1, 2),
            Some(255),
            "colour is already in place half way through the fade"
        );
        assert_eq!(engine.render().slot(1, 1), Some(128), "dimmer still fades");
    }

    #[test]
    fn per_attribute_timing_reaches_the_output() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At 0").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R 0").unwrap();
        engine.execute("Store Cue 1 Fade 0").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R Full").unwrap();
        engine.execute("Store Cue 2 Fade 2").unwrap();
        engine.execute("Cue 2 Timing ColorAdd_R Fade 0.5").unwrap();
        engine
            .execute("Cue 2 Timing Dimmer Delay 1.5 Curve Snap")
            .unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.execute("Go Sequence 1").unwrap();

        engine.tick(start + Duration::from_secs(1));
        assert_eq!(
            engine.render().slot(1, 2),
            Some(255),
            "colour arrives first"
        );
        assert_eq!(
            engine.render().slot(1, 1),
            Some(0),
            "the dimmer is still held"
        );

        engine.tick(start + Duration::from_secs(2));
        assert_eq!(engine.render().slot(1, 1), Some(255), "the dimmer snapped");
    }

    #[test]
    fn per_attribute_timing_round_trips_through_save_and_load() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1 Fade 2").unwrap();
        engine
            .execute("Cue 1 Timing ColorAdd_R Fade 0.5 Curve EaseOut")
            .unwrap();
        engine.execute("Cue 1 Stagger 0.2").unwrap();

        let path = std::env::temp_dir().join(format!(
            "openlight-{}-timing.olshow.json",
            std::process::id()
        ));
        engine
            .execute(&format!("Save \"{}\"", path.display()))
            .unwrap();
        let show = Show::load(&path).unwrap();
        let cue = &show.sequences[0].cues[0];
        let timing = cue.timing.get("ColorAdd_R").expect("timing is stored");
        assert_eq!(timing.fade, Some(0.5));
        assert_eq!(timing.curve, Curve::EaseOut);
        assert_eq!(cue.stagger, 0.2);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("oldesk"));
        let _ = std::fs::remove_file(crate::journal::path_for(&path, ".bak"));
        let _ = std::fs::remove_file(crate::journal::path_for(&path, ".autosave"));
    }

    #[test]
    fn timing_off_restores_the_cue_fade() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1 Fade 2").unwrap();
        engine.execute("Cue 1 Timing Dimmer Fade 0.5").unwrap();

        let listing = engine.execute("Sequence 1").unwrap().lines.join("\n");
        assert!(listing.contains("Timing"), "{listing}");
        assert!(listing.contains("0.5"), "{listing}");

        engine.execute("Cue 1 Timing Dimmer Off").unwrap();
        assert!(
            engine.show.sequences[0].cues[0].timing.is_empty(),
            "an override that overrides nothing is dropped"
        );
        let line = engine
            .execute("Cue 1 Timing Dimmer")
            .unwrap()
            .lines
            .join("\n");
        assert!(line.contains("follows the cue fade"), "{line}");
    }

    #[test]
    fn timing_an_attribute_the_fixtures_lack_is_rejected() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        let error = engine.execute("Cue 1 Timing Pan Fade 1").unwrap_err();
        assert!(error.message.contains("Pan"), "{error}");
        assert!(engine.show.sequences[0].cues[0].timing.is_empty());
    }

    #[test]
    fn timing_a_running_cue_does_not_disturb_the_run() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At 0").unwrap();
        engine.execute("Store Cue 1 Fade 0").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 2 Fade 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start + Duration::from_millis(500));
        assert_eq!(engine.render().slot(1, 1), Some(128));

        engine.execute("Cue 2 Timing Dimmer Fade 0").unwrap();
        engine.tick(start + Duration::from_millis(500));
        assert_eq!(
            engine.render().slot(1, 1),
            Some(128),
            "the run keeps the plan it started with"
        );
    }

    #[test]
    fn home_is_the_base_below_programmer_and_playback() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 Default ColorAdd_R 50").unwrap();
        assert_eq!(engine.render().slot(1, 2), Some(128));
        let sheet = engine.execute("Sheet").unwrap().lines.join("\n");
        assert!(sheet.contains("H Red 50%"), "{sheet}");

        engine.execute("Fixture 1 At Full").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(255));
        assert_eq!(engine.render().slot(1, 2), Some(128), "home still colours");
        assert!(engine
            .execute("Sheet")
            .unwrap()
            .lines
            .join("\n")
            .contains("P Dimmer 100%"));

        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start);
        let sheet = engine.execute("Sheet").unwrap().lines.join("\n");
        assert!(sheet.contains("Pb Dimmer 100%"), "{sheet}");
        assert!(sheet.contains("H Red 50%"), "{sheet}");
    }

    #[test]
    fn home_recalls_the_defaults_into_the_programmer() {
        let mut engine = engine();
        engine.execute("Fixture 1 Default Dimmer 60").unwrap();
        engine.execute("Fixture 1 Default ColorAdd_R 25").unwrap();
        engine.execute("Fixture 1 Home").unwrap();
        assert_eq!(engine.desk.programmer.get(1, "Dimmer"), Some(0.6));
        assert_eq!(engine.desk.programmer.get(1, "ColorAdd_R"), Some(0.25));

        let listing = engine
            .execute("Fixture 1 Default")
            .unwrap()
            .lines
            .join("\n");
        assert!(
            listing.contains("Dimmer") && listing.contains("stored"),
            "{listing}"
        );

        engine.execute("Fixture 1 Default Dimmer Off").unwrap();
        assert!(!engine.show.fixture(1).unwrap().home.contains_key("Dimmer"));
        let listing = engine
            .execute("Fixture 1 Default")
            .unwrap()
            .lines
            .join("\n");
        assert!(listing.contains("profile"), "{listing}");
    }

    #[test]
    fn home_recalled_into_a_cue_survives_clear_values() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 Default ColorAdd_R 50").unwrap();
        engine.execute("Fixture 1 Home").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start);
        assert_eq!(engine.render().slot(1, 2), Some(128));
    }

    #[test]
    fn default_rejects_an_attribute_the_mode_lacks() {
        let mut engine = engine();
        let error = engine.execute("Fixture 1 Default Pan 50").unwrap_err();
        assert!(error.message.contains("Pan"), "{error}");
        assert!(engine.show.fixture(1).unwrap().home.is_empty());
    }

    #[test]
    fn group_default_and_home_apply_to_every_member() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Group 1").unwrap();
        engine.execute("Group 1 Default Dimmer 30").unwrap();
        assert_eq!(
            engine.show.fixture(1).unwrap().home.get("Dimmer"),
            Some(&0.3)
        );
        engine.execute("Group 1 Home").unwrap();
        assert_eq!(engine.desk.programmer.get(1, "Dimmer"), Some(0.3));
    }

    #[test]
    fn timecode_drives_cues_from_the_engine_clock() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At 0").unwrap();
        engine.execute("Store Cue 2 Fade 0").unwrap();
        engine.execute("Clear All").unwrap();

        engine.execute("Store Timecode 1 \"Song\"").unwrap();
        engine
            .execute("Timecode 1 Add 00:00:00:00 Sequence 1 Cue 1")
            .unwrap();
        engine
            .execute("Timecode 1 Add 00:00:01:00 Sequence 1 Cue 2")
            .unwrap();
        engine.execute("Timecode 1 Start").unwrap();

        assert!(engine.tick(start + Duration::from_millis(10)));
        assert_eq!(
            engine.render().slot(1, 1),
            Some(255),
            "the event at frame zero starts cue 1"
        );
        engine.tick(start + Duration::from_secs(2));
        assert_eq!(
            engine.render().slot(1, 1),
            Some(0),
            "the event at one second starts cue 2"
        );

        let listing = engine.execute("Timecode 1").unwrap().lines.join("\n");
        assert!(listing.contains("00:00:01:00"), "{listing}");
        assert!(listing.contains("running"), "{listing}");
    }

    #[test]
    fn timecode_pauses_stops_and_locates() {
        let mut engine = engine();
        let start = engine.clock();
        engine.tick(start);
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Store Timecode 1 \"Song\" Fps 25").unwrap();
        engine
            .execute("Timecode 1 Add 00:00:01:00 Sequence 1 Cue 1")
            .unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Timecode 1 Start").unwrap();
        engine.tick(start + Duration::from_millis(500));

        let paused = engine.execute("Timecode 1 Pause").unwrap();
        assert!(paused.lines[0].contains("00:00:00:12"), "{paused:?}");
        assert!(!engine.timecode.is_running());
        engine.tick(start + Duration::from_secs(5));
        assert_eq!(
            engine.render().slot(1, 1),
            Some(0),
            "a paused track does not fire"
        );

        engine.execute("Timecode 1 Start").unwrap();
        engine.tick(start + Duration::from_secs(6));
        assert_eq!(
            engine.render().slot(1, 1),
            Some(255),
            "resuming keeps the position and passes the event"
        );

        engine.execute("Timecode 1 Stop").unwrap();
        let stopped = engine.execute("Timecode 1").unwrap().lines.join("\n");
        assert!(stopped.contains("stopped 00:00:00:00"), "{stopped}");

        let located = engine.execute("Timecode 1 At 00:00:01:00").unwrap();
        assert!(located.lines[0].contains("00:00:01:00"), "{located:?}");
    }

    #[test]
    fn timecode_commands_report_bad_input() {
        let mut engine = engine();
        assert!(
            engine.execute("Timecode 1").is_ok(),
            "a listing is a message"
        );
        assert!(engine
            .execute("Timecode 1 Add 00:00:00:00 Sequence 1 Cue 1")
            .is_err());
        engine.execute("Store Timecode 1").unwrap();
        assert!(engine
            .execute("Timecode 1 Add 00:00:00:00 Sequence 1 Cue 1")
            .is_err());

        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        assert!(engine
            .execute("Timecode 1 Add 00:00:00:00 Sequence 1 Cue 2")
            .is_err());
        engine
            .execute("Timecode 1 Add 00:00:00:00 Sequence 1 Cue 1")
            .unwrap();
        assert!(
            engine
                .execute("Timecode 1 Add 00:00:00:00 Sequence 1 Cue 1")
                .is_err(),
            "duplicate positions are rejected"
        );
        assert!(
            engine
                .execute("Timecode 1 Add 00:00:00:25 Sequence 1 Cue 1")
                .is_err(),
            "25 fps has no frame 25"
        );
        assert!(engine.execute("Store Timecode 2 Fps 61").is_err());
        assert!(engine.execute("Timecode 1 Remove 00:00:09:00").is_err());
        assert!(engine.execute("Timecode 1 Remove 00:00:00:00").is_ok());
        assert!(engine.execute("Delete Timecode 1").is_ok());
        assert!(engine.execute("Timecode 1").is_ok());
    }

    #[test]
    fn timecode_shows_survive_save_and_load() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Store Timecode 1 \"Song\" Fps 30").unwrap();
        engine
            .execute("Timecode 1 Add 00:00:02:00 Sequence 1 Cue 1")
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "openlight-phase4-{}.olshow.json",
            std::process::id()
        ));
        engine
            .execute(&format!("Save \"{}\"", path.display()))
            .unwrap();

        let mut other = Engine::empty();
        other
            .execute(&format!("Load \"{}\"", path.display()))
            .unwrap();
        assert_eq!(other.show.timecodes.len(), 1);
        assert_eq!(other.show.timecodes[0].fps, 30);
        assert_eq!(other.show.timecodes[0].events[0].at, "00:00:02:00");
        assert!(!other.timecode.is_running(), "the player is not saved");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn status_reports_timecode_and_undo_depth() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 30").unwrap();
        engine.execute("Store Timecode 1").unwrap();
        let status = engine.execute("Status").unwrap().lines.join("\n");
        assert!(status.contains("Undo: 2 steps"), "{status}");
        assert!(status.contains("Timecode: idle"), "{status}");
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

    #[test]
    fn blackout_is_a_render_master_and_never_an_edit() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 50").unwrap();
        let lit = engine.render().slot(1, 1);
        assert!(lit.unwrap() > 0, "the fixture is lit");

        engine.execute("Blackout").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(0));
        assert_eq!(
            engine.desk.programmer.get(1, "Dimmer"),
            Some(0.5),
            "blackout never touches the programmer"
        );
        let undid = engine.execute("Undo").unwrap();
        assert!(
            !undid.lines[0].contains("Blackout"),
            "blackout is not an undo step: {undid:?}"
        );

        // Undo stepped past the blackout and back to the edit before it.
        assert!(engine.desk.programmer.is_empty());
        engine.execute("Fixture 1 At 50").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(0), "still blacked out");
        engine.execute("Blackout Off").unwrap();
        assert_eq!(engine.render().slot(1, 1), lit, "the same look returns");
    }

    #[test]
    fn a_lock_refuses_mutations_but_still_answers_questions() {
        let mut engine = engine();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Lock 4711").unwrap();

        for line in [
            "Fixture 1 At 50",
            "Store Cue 2",
            "Delete Cue 1",
            "Patch Fixture 9 At 1.101",
            "New",
            "Load \"x.olshow\"",
            "Master Grand At 50",
        ] {
            let error = engine.execute(line).unwrap_err();
            assert!(error.to_string().contains("locked"), "{line}: {error}");
            assert_eq!(
                error.hint.as_deref(),
                Some("`Unlock <code>` releases it"),
                "{line}"
            );
        }

        assert!(engine.execute("Status").is_ok());
        assert!(engine.execute("Sheet").is_ok());
        assert!(engine.execute("Sequence").is_ok());
        assert!(
            engine
                .execute("Status")
                .unwrap()
                .lines
                .iter()
                .any(|line| line.contains("Lock: on")),
            "status names the lock"
        );

        // A wrong code leaves the desk locked, and lifting a blackout works.
        assert!(engine.execute("Unlock 0000").is_err());
        assert!(engine.execute("Fixture 1 At 50").is_err());
        engine.execute("Blackout Off").unwrap();

        engine.execute("Unlock 4711").unwrap();
        engine.execute("Fixture 1 At 50").unwrap();
    }

    #[test]
    fn a_follow_chain_runs_without_commands_between_ticks() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 1 Fade 0").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 2 Follow 1 Fade 0").unwrap();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 3 Follow 0 Fade 0").unwrap();

        let start = engine.clock();
        engine.execute("Go Sequence 1").unwrap();
        engine.tick(start);
        assert_eq!(
            engine.playback.run(Target::Sequence(1)).unwrap().cue_number,
            1.0
        );
        engine.tick(start + Duration::from_secs(1));
        assert_eq!(
            engine.playback.run(Target::Sequence(1)).unwrap().cue_number,
            2.0,
            "cue 2 follows one second after cue 1 settles"
        );
        engine.tick(start + Duration::from_secs(1) + Duration::from_millis(20));
        assert_eq!(
            engine.playback.run(Target::Sequence(1)).unwrap().cue_number,
            3.0,
            "cue 3 follows cue 2 immediately"
        );
    }

    #[test]
    fn a_timecoded_cue_is_not_chained_again_on_the_same_tick() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 1 Fade 0").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 2 Follow 0 Fade 0").unwrap();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Store Cue 3 Follow 0 Fade 0").unwrap();
        engine.execute("Store Timecode 1 Fps 25").unwrap();
        engine
            .execute("Timecode 1 Add 00:00:01:00 Sequence 1 Cue 1")
            .unwrap();

        let start = engine.clock();
        engine.execute("Timecode 1 Start").unwrap();
        engine.tick(start + Duration::from_secs(1));
        assert_eq!(
            engine.playback.run(Target::Sequence(1)).unwrap().cue_number,
            1.0,
            "the timecoded cue sits on its cue for this tick"
        );
        engine.tick(start + Duration::from_secs(1) + Duration::from_millis(20));
        assert_eq!(
            engine.playback.run(Target::Sequence(1)).unwrap().cue_number,
            2.0,
            "its chain starts on the next tick"
        );
    }

    #[test]
    fn a_trigger_edit_is_one_undo_step_and_keeps_the_running_cue() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 1 Fade 0").unwrap();
        engine.execute("Fixture 1 At 60").unwrap();
        engine.execute("Store Cue 2 Fade 2").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        let running = engine.playback.run(Target::Sequence(1)).unwrap().cue_number;

        engine.execute("Cue 2 Trigger Follow 1.5").unwrap();
        let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
        assert_eq!(cue.trigger, Trigger::Follow);
        assert_eq!(cue.trigger_time, 1.5);
        assert_eq!(
            engine.playback.run(Target::Sequence(1)).unwrap().cue_number,
            running,
            "a trigger edit does not restart a running cue"
        );

        // A `Go` keeps the stored time, so toggling does not lose it.
        engine.execute("Cue 2 Trigger Go").unwrap();
        let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
        assert_eq!(cue.trigger, Trigger::Go);
        assert_eq!(cue.trigger_time, 1.5);

        engine.execute("Undo").unwrap();
        let cue = engine.show.sequence(1).unwrap().cue(2.0).unwrap();
        assert_eq!(cue.trigger, Trigger::Follow, "one undo step per edit");
    }

    #[test]
    fn executor_modes_drive_the_fader_by_hand() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 1 Fade 0").unwrap();
        engine.execute("Assign Executor 1 Sequence 1").unwrap();
        engine.execute("Executor 1 Mode Fader").unwrap();
        assert_eq!(engine.show.executor(1).unwrap().mode, ExecutorMode::Fader);

        engine.execute("Executor 1 At 60").unwrap();
        let run = engine.playback.run(Target::Executor(1)).unwrap();
        assert_eq!(run.cue_number, 1.0, "a fader on an idle list starts cue 1");
        assert!(run.is_paused(), "the fader holds the crossfade");
        assert_eq!(
            run.levels_at(engine.clock()).get(1, "Dimmer"),
            Some(0.24),
            "60% of the way from nothing to 40%"
        );

        let lines = engine.execute("Executor 1").unwrap().lines;
        assert!(
            lines.iter().any(|line| line.contains("Fader")),
            "the listing names the mode: {lines:#?}"
        );
    }

    #[test]
    fn solo_silences_the_other_executor_and_flash_restores_the_fader() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Assign Executor 1 Sequence 1").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 2 Sequence 2").unwrap();
        engine.execute("Assign Executor 2 Sequence 2").unwrap();
        engine.execute("Go Executor 1").unwrap();
        engine.execute("Go Executor 2").unwrap();
        assert_eq!(engine.playback.output().get(1, "Dimmer"), Some(1.0));

        engine.execute("Executor 1 Solo").unwrap();
        assert_eq!(
            engine.playback.output().get(1, "Dimmer"),
            Some(0.4),
            "the soloed executor is the only one left"
        );
        engine.execute("Executor 1 Solo Off").unwrap();
        assert_eq!(engine.playback.output().get(1, "Dimmer"), Some(1.0));

        engine.execute("Release Executor 2").unwrap();
        engine.execute("Executor 1 At 50").unwrap();
        assert_eq!(engine.playback.output().get(1, "Dimmer"), Some(0.2));
        engine.execute("Executor 1 Flash").unwrap();
        assert_eq!(
            engine.playback.output().get(1, "Dimmer"),
            Some(0.4),
            "flash ignores the fader"
        );
        engine.execute("Executor 1 Flash Off").unwrap();
        assert_eq!(
            engine.playback.output().get(1, "Dimmer"),
            Some(0.2),
            "releasing restores it"
        );
    }

    fn journal_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("openlight-{}-{name}.journal", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn the_journal_records_changes_and_nothing_else() {
        let path = journal_path("records");
        let mut engine = engine();
        engine.set_journal(Some(crate::journal::Journal::open(path.clone()).unwrap()));

        engine
            .execute("Fixture 1 At 50; Store Cue 1; Clear Values")
            .unwrap();
        let entries = crate::journal::read(&path).unwrap().entries;
        assert_eq!(entries.len(), 3, "{entries:#?}");
        assert_eq!(entries[0].line, "Fixture 1 At 50");
        assert_eq!(entries[1].line, "Store Cue 1");
        assert_eq!(entries[2].line, "Clear Values");
        assert_eq!(entries[0].kind, crate::journal::EntryKind::Programmer);
        assert_eq!(entries[1].kind, crate::journal::EntryKind::Show);

        // Questions, listings and playback moves change no persisted state.
        for line in ["Sheet", "Status", "Sequence", "Palette"] {
            engine.execute(line).unwrap();
        }
        assert_eq!(
            crate::journal::read(&path).unwrap().entries.len(),
            3,
            "a question is not an edit"
        );

        // `New` replaces the document, so it has to be replayable too.
        engine.execute("New Fresh").unwrap();
        let entries = crate::journal::read(&path).unwrap().entries;
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[3].line, "New Fresh");
        let _ = std::fs::remove_file(&path);
    }

    /// A show path no checkpoint can name, for tests that replay a bare tail.
    const NO_SHOW: &str = "openlight-replay-test.olshow";

    #[test]
    fn replaying_the_journal_reproduces_show_and_programmer() {
        let path = journal_path("replay");
        let mut session = engine();
        session.set_journal(Some(crate::journal::Journal::open(path.clone()).unwrap()));
        session.execute("Fixture 1 At 70; Store Cue 2").unwrap();
        let show = session.show.clone();
        let programmer = session.desk.programmer.clone();
        drop(session);

        let tail = crate::journal::read(&path).unwrap();
        assert_eq!(tail.entries.len(), 2);
        assert_eq!(tail.incomplete, 0);

        let mut recovered = engine();
        let report = recovered.recover(&tail, Path::new(NO_SHOW), ReplayFrom::File);
        assert_eq!(report.applied, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(recovered.show, show);
        assert_eq!(recovered.desk.programmer, programmer);
        assert_eq!(
            recovered.history.undo_depth(),
            0,
            "replay is not a pile of undo steps"
        );

        // Replaying inside a live session must not journal the replay itself.
        recovered.set_journal(Some(crate::journal::Journal::open(path.clone()).unwrap()));
        let before = crate::journal::read(&path).unwrap().entries.len();
        recovered.recover(&tail, Path::new(NO_SHOW), ReplayFrom::File);
        assert_eq!(crate::journal::read(&path).unwrap().entries.len(), before);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replay_skips_a_statement_that_no_longer_applies() {
        use crate::journal::{Entry, EntryKind, Tail};
        let tail = Tail {
            entries: vec![
                Entry {
                    rev: 1,
                    at: "2026-11-19T21:04:12Z".into(),
                    line: "Fixture 99 At 50".into(),
                    kind: EntryKind::Programmer,
                },
                Entry {
                    rev: 2,
                    at: "2026-11-19T21:04:13Z".into(),
                    line: "Fixture 1 At 30".into(),
                    kind: EntryKind::Programmer,
                },
            ],
            incomplete: 1,
        };
        let mut engine = engine();
        let report = engine.recover(&tail, Path::new(NO_SHOW), ReplayFrom::File);
        assert_eq!(report.applied, 1);
        assert_eq!(report.skipped.len(), 1, "the missing fixture is not fatal");
        assert!(report.skipped[0].contains("Fixture 99"));
        assert_eq!(report.incomplete, 1);
        assert!(report.recovered_anything());
        assert_eq!(engine.desk.programmer.get(1, "Dimmer"), Some(0.3));
    }

    #[test]
    fn a_break_in_the_chain_stops_replay() {
        use crate::journal::{Entry, EntryKind, Tail};
        let entry = |rev: u64, line: &str| Entry {
            rev,
            at: "2026-11-19T21:04:12Z".into(),
            line: line.into(),
            kind: EntryKind::Programmer,
        };
        let tail = Tail {
            entries: vec![entry(1, "Fixture 1 At 20"), entry(3, "Fixture 1 At 90")],
            incomplete: 0,
        };
        let mut engine = engine();
        let report = engine.recover(&tail, Path::new(NO_SHOW), ReplayFrom::File);
        assert_eq!(report.applied, 1);
        assert_eq!(report.stopped, Some(3));
        assert_eq!(
            engine.desk.programmer.get(1, "Dimmer"),
            Some(0.2),
            "the entry past the break is not replayed"
        );
    }

    #[test]
    fn a_save_defines_where_replay_starts() {
        let journal = journal_path("baseline");
        let show_path =
            journal.with_file_name(format!("openlight-{}-baseline.olshow", std::process::id()));
        let _ = std::fs::remove_file(&show_path);
        let _ = std::fs::remove_file(show_path.with_extension("oldesk"));

        let mut session = engine();
        session.set_journal(Some(
            crate::journal::Journal::open(journal.clone()).unwrap(),
        ));
        session.execute("Fixture 1 At 40").unwrap();
        session.execute("Fixture 1 At 90").unwrap();
        // `Undo` is not journaled. Replaying the two edits would put the
        // programmer back at 90 and disagree with the file saved at 40.
        session.execute("Undo").unwrap();
        session
            .execute(&format!("Save \"{}\"", show_path.display()))
            .unwrap();
        session.execute("Fixture 1 At 20").unwrap();
        drop(session);

        let tail = crate::journal::read(&journal).unwrap();
        assert_eq!(tail.entries.len(), 4, "{tail:#?}");
        assert_eq!(tail.entries[2].kind, crate::journal::EntryKind::Checkpoint);
        assert_eq!(tail.baseline(&show_path, ReplayFrom::File).rev, 3);

        let (mut reopened, _) = Engine::open(&show_path).unwrap();
        assert_eq!(
            reopened.desk.programmer.get(1, "Dimmer"),
            Some(0.4),
            "the file holds the undone value"
        );
        let report = reopened.recover(&tail, &show_path, ReplayFrom::File);
        assert_eq!(
            report.applied, 1,
            "only the edit after the save: {report:?}"
        );
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(reopened.desk.programmer.get(1, "Dimmer"), Some(0.2));

        let _ = std::fs::remove_file(&journal);
        let _ = std::fs::remove_file(&show_path);
        let _ = std::fs::remove_file(show_path.with_extension("oldesk"));
    }

    #[test]
    fn a_snapshot_records_the_journal_head_when_no_journal_is_open() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "openlight-{}-journal-head.olshow.autosave",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let mut engine = engine();
        engine.set_journal_head(7);
        engine.enable_autosave(path.clone(), Duration::from_secs(30));
        let start = engine.clock();
        engine.tick(start + Duration::from_secs(31));

        let snapshot = crate::journal::Snapshot::load(&path).unwrap().unwrap();
        assert_eq!(snapshot.rev, Some(7));
        assert_eq!(snapshot.replay_from(), ReplayFrom::Snapshot(7));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_channel_set_stores_its_physical_midpoint() {
        let mut engine = Engine::new(demo_moving_show());
        engine.execute("Fixture 1 Gobo 2").unwrap();
        let channel = engine.show.fixture_types[0].modes[0]
            .channel_for("Gobo")
            .unwrap();
        assert_eq!(
            engine.desk.programmer.get(1, "Gobo"),
            channel.level_for_set(SetSelector::Index(2))
        );
        assert_eq!(
            engine.render().slot(1, 3),
            Some(32),
            "midpoint of DMX 16..47"
        );
    }

    #[test]
    fn a_channel_set_name_is_case_insensitive_and_errors_name_the_choices() {
        let mut engine = Engine::new(demo_moving_show());
        engine.execute(r#"Fixture 1 Gobo "rotate cw""#).unwrap();
        assert_eq!(
            engine.render().slot(1, 3),
            Some(72),
            "midpoint of DMX 48..95"
        );

        let error = engine.execute("Fixture 1 Gobo 99").unwrap_err();
        assert!(error.message.contains("3 sets"), "{error}");
        assert!(error.message.contains("99"), "{error}");

        let error = engine.execute(r#"Fixture 1 Gobo "Gobo""#).unwrap_err();
        assert!(error.message.contains("no set `Gobo`"), "{error}");
        assert!(error.hint.unwrap().contains("Gobo 1"));
    }

    #[test]
    fn listing_a_channel_shows_its_sets_and_marks_the_current_one() {
        let mut engine = Engine::new(demo_moving_show());
        engine.execute("Fixture 1 Gobo 2").unwrap();
        let text = engine.execute("Fixture 1 Gobo").unwrap().lines.join("\n");
        assert!(text.contains("Fixture 1 Gobo (16-bit: no)"), "{text}");
        assert!(text.contains("Gobo 1"), "{text}");
        assert!(text.contains("Rotate CW"), "{text}");
        assert!(text.contains("current: Gobo 2 (DMX 32)"), "{text}");
    }

    #[test]
    fn a_group_set_applies_to_members_with_the_attribute() {
        let mut show = demo_moving_show();
        show.fixture_types.push(demo_fixture_type());
        show.fixtures.push(Fixture {
            number: 2,
            name: "Demo Wash".into(),
            type_key: "OpenLight Demo Wash".into(),
            mode: "4ch".into(),
            universe: 1,
            address: 4,
            home: Default::default(),
        });
        show.groups.push(Group {
            number: 1,
            name: "All".into(),
            fixtures: vec![1, 2],
        });
        let mut engine = Engine::new(show);
        let outcome = engine.execute(r#"Group 1 Gobo "Gobo 2""#).unwrap();
        let text = outcome.lines.join("\n");
        assert!(engine.desk.programmer.get(1, "Gobo").is_some());
        assert!(text.contains("Warning: fixture 2 has no `Gobo`"), "{text}");
        assert_eq!(engine.render().slot(1, 3), Some(32));
    }

    #[test]
    fn a_sixteen_bit_attribute_writes_two_bytes() {
        let mut engine = Engine::new(demo_moving_show());
        engine.execute("Fixture 1 At Pan 50").unwrap();
        let frame = engine.render();
        assert_eq!(frame.slot(1, 1), Some(127), "coarse");
        assert_eq!(frame.slot(1, 2), Some(255), "fine");
    }

    #[test]
    fn clone_copies_type_mode_home_and_numbers_from_the_next_free() {
        let mut engine = Engine::new(demo_show());
        engine.execute("Patch Fixture 2 At 1.201").unwrap();
        engine.show.fixtures[0].home.insert("Dimmer".into(), 0.4);
        let outcome = engine.execute("Clone Fixture 1 At 1.101 Count 8").unwrap();
        assert!(outcome.lines[0].contains("8 new fixtures"), "{outcome:?}");

        let clones: Vec<&Fixture> = engine
            .show
            .fixtures
            .iter()
            .filter(|fixture| fixture.address >= 101 && fixture.address < 200)
            .collect();
        assert_eq!(clones.len(), 8);
        assert_eq!((clones[0].number, clones[7].number), (3, 10));
        assert_eq!((clones[0].address, clones[7].address), (101, 129));
        assert_eq!(clones[0].name, "Demo Wash 2");
        assert_eq!(clones[7].name, "Demo Wash 9");
        assert_eq!(clones[0].type_key, "OpenLight Demo Wash");
        assert_eq!(clones[0].mode, "4ch");
        assert_eq!(clones[0].home.get("Dimmer"), Some(&0.4));
        assert!(engine.show.validate().is_empty());
    }

    #[test]
    fn a_clone_that_would_overlap_changes_nothing() {
        let mut engine = Engine::new(demo_show());
        let before = engine.show.clone();

        let error = engine.execute("Clone Fixture 1 At 1.002").unwrap_err();
        assert!(error.message.contains("overlap fixture 1"), "{error}");
        assert_eq!(engine.show, before);

        let error = engine
            .execute("Clone Fixture 1 At 1.101 Count 2 Step 2")
            .unwrap_err();
        assert!(error.message.contains("overlap clone 1"), "{error}");
        assert_eq!(engine.show, before);

        let error = engine.execute("Clone Fixture 1 At 1.510").unwrap_err();
        assert!(error.message.contains("needs 4 slots"), "{error}");
        assert_eq!(engine.show, before);
    }

    #[test]
    fn patch_check_reports_usage_gaps_and_overlaps() {
        let mut engine = Engine::new(demo_show());
        let before = engine.revision;
        let text = engine.execute("Patch Check").unwrap().lines.join("\n");
        assert!(text.contains("1 fixture, 4 slots, universes 1"), "{text}");
        assert!(text.contains("no overlaps"), "{text}");
        assert!(
            text.contains("universe 1: 4 of 512 slots used, largest gap 508 slots at 1.005"),
            "{text}"
        );
        assert_eq!(engine.revision, before, "a check is not a change");

        let mut show = demo_show();
        show.fixtures.push(Fixture {
            number: 2,
            name: "Overlap".into(),
            type_key: "OpenLight Demo Wash".into(),
            mode: "4ch".into(),
            universe: 1,
            address: 2,
            home: Default::default(),
        });
        let mut engine = Engine::new(show);
        let text = engine.execute("Patch Check").unwrap().lines.join("\n");
        assert!(text.contains("overlap: fixtures 1 and 2"), "{text}");

        let mut show = demo_show();
        show.fixtures[0].address = 510;
        let mut engine = Engine::new(show);
        let text = engine.execute("Patch Check").unwrap().lines.join("\n");
        assert!(
            text.contains("needs 4 slots but universe 1 has 512 slots"),
            "{text}"
        );
    }

    #[test]
    fn patch_import_replaces_the_rig_and_reads_a_quoted_comma() {
        let path = temp_path("import.csv");
        std::fs::write(
            &path,
            "number,name,type,mode,universe,address\n\
             1,First,OpenLight Demo Wash,4ch,1,1\n\
             2,\"Second, warm\",OpenLight Demo Wash,4ch,1,5\n",
        )
        .unwrap();
        let mut engine = Engine::new(demo_show());
        engine
            .execute(&format!(r#"Patch Import "{}""#, path.display()))
            .unwrap();
        assert_eq!(engine.show.fixtures.len(), 2);
        assert_eq!(engine.show.fixtures[1].name, "Second, warm");
        assert_eq!(engine.show.fixtures[1].address, 5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_bad_patch_import_leaves_every_fixture_in_place() {
        let path = temp_path("bad-import.csv");
        std::fs::write(
            &path,
            "number,name,type,mode,universe,address\n\
             1,First,OpenLight Demo Wash,4ch,1,1\n\
             2,Second,OpenLight Demo Wash,4ch,1,510\n",
        )
        .unwrap();
        let mut engine = Engine::new(demo_show());
        engine.execute("Fixture 1 At 50").unwrap();
        let before = engine.show.clone();
        let error = engine
            .execute(&format!(r#"Patch Import "{}""#, path.display()))
            .unwrap_err();
        assert!(error.message.starts_with("line 3:"), "{error}");
        assert!(error.message.contains("512 slots"), "{error}");
        assert_eq!(engine.show, before);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_unknown_type_in_a_patch_import_names_the_import_command() {
        let path = temp_path("unknown-type.csv");
        std::fs::write(
            &path,
            "number,name,type,mode,universe,address\n1,A,Nope,4ch,1,1\n",
        )
        .unwrap();
        let mut engine = Engine::new(demo_show());
        let error = engine
            .execute(&format!(r#"Patch Import "{}""#, path.display()))
            .unwrap_err();
        assert!(error.message.contains("unknown fixture type"), "{error}");
        assert!(error.hint.unwrap().contains("Import Gdtf"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn report_patch_round_trips_through_import() {
        let mut engine = Engine::new(demo_show());
        let path = temp_path("patch-roundtrip.csv");
        engine
            .execute(&format!(r#"Report Patch "{}""#, path.display()))
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("number,name,type,mode,universe,address"),
            "{text}"
        );
        assert!(
            text.contains("1,Demo Wash,OpenLight Demo Wash,4ch,1,1"),
            "{text}"
        );

        let mut fresh = Engine::new(Show::new());
        fresh.show.fixture_types.push(demo_fixture_type());
        fresh
            .execute(&format!(r#"Patch Import "{}""#, path.display()))
            .unwrap();
        assert_eq!(fresh.show.fixtures, engine.show.fixtures);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn report_cues_writes_csv_or_the_listing() {
        let mut engine = Engine::new(demo_show());
        engine.execute(r#"Store Cue 1 "Warm" Fade 2"#).unwrap();

        let csv = temp_path("cues.csv");
        engine
            .execute(&format!(r#"Report Cues 1 "{}""#, csv.display()))
            .unwrap();
        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(
            text.starts_with("sequence,number,name,fade,delay,trigger,mib,blocked"),
            "{text}"
        );
        assert!(text.contains("1,1,Warm,2.0,0.0,Go,no,no"), "{text}");

        let txt = temp_path("cues.txt");
        engine
            .execute(&format!(r#"Report Cues 1 "{}""#, txt.display()))
            .unwrap();
        let text = std::fs::read_to_string(&txt).unwrap();
        assert!(text.contains("Sequence 1"), "{text}");
        let _ = std::fs::remove_file(&csv);
        let _ = std::fs::remove_file(&txt);
    }

    #[test]
    fn report_show_contains_every_listing_and_changes_nothing() {
        let mut engine = Engine::new(demo_show());
        engine.execute("Fixture 1 At 50").unwrap();
        let before_show = engine.show.clone();
        let before_revision = engine.revision;
        let before_selection = engine.desk.selection.clone();

        let path = temp_path("show-report.md");
        engine
            .execute(&format!(r#"Report Show "{}""#, path.display()))
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Untitled Show"), "{text}");
        for heading in [
            "## Sheet",
            "## Playback",
            "## Pools",
            "## Timecode",
            "## Status",
        ] {
            assert!(text.contains(heading), "{heading} missing from {text}");
        }
        let sheet = engine.execute("Sheet").unwrap().lines.join("\n");
        assert!(text.contains(&sheet), "the sheet listing is included");
        let sequences = engine.execute("Sequence").unwrap().lines.join("\n");
        assert!(
            text.contains(&sequences),
            "the sequence listing is included"
        );

        assert_eq!(engine.show, before_show);
        assert_eq!(engine.revision, before_revision);
        assert_eq!(engine.desk.selection, before_selection);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reports_do_not_journal_or_bump_the_revision() {
        let mut engine = Engine::new(demo_show());
        let journal = temp_path("report.journal");
        let _ = std::fs::remove_file(&journal);
        engine.set_journal(Some(
            crate::journal::Journal::open(journal.clone()).unwrap(),
        ));
        let before = engine.revision;

        let path = temp_path("status-report.txt");
        engine
            .execute(&format!(r#"Report Status "{}""#, path.display()))
            .unwrap();
        assert_eq!(engine.revision, before);
        assert_eq!(engine.journal().unwrap().entries(), 0);
        assert_eq!(engine.journal().unwrap().rev(), 0);

        engine.set_journal(None);
        let _ = std::fs::remove_file(&journal);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn blind_hides_the_programmer_but_not_playback() {
        let mut engine = engine();
        engine.execute("Blind").unwrap();
        engine.execute("Fixture 1 At Full").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(0));
        assert!(engine
            .execute("Status")
            .unwrap()
            .lines
            .iter()
            .any(|line| line.contains("Blind: on")));

        engine.execute("Blind Off").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(255));

        // A running cue keeps driving the rig while the desk is blind.
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();
        engine.execute("Blind").unwrap();
        assert_eq!(
            engine.render().slot(1, 1),
            Some(255),
            "playback is not blind"
        );
    }

    #[test]
    fn a_blind_store_captures_the_programmer() {
        let mut engine = engine();
        engine.execute("Blind").unwrap();
        engine.execute("Fixture 1 At 40").unwrap();
        engine.execute("Store Cue 1").unwrap();
        let cue = engine.show.sequence(1).unwrap().cue(1.0).unwrap();
        assert_eq!(cue.levels.get(1, "Dimmer"), Some(0.4));
    }

    #[test]
    fn preview_shows_a_cue_without_reaching_the_stage() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Clear Values").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(0));

        let outcome = engine.execute("Preview Cue 1").unwrap();
        let message = outcome.lines.join("\n");
        assert!(message.contains("cue 1 in sequence 1"), "{message}");
        assert_eq!(engine.preview_frame().unwrap().slot(1, 1), Some(255));
        assert_eq!(
            engine.render().slot(1, 1),
            Some(0),
            "a preview never becomes output"
        );

        // The frame is exactly the cue rendered with no programmer or masters.
        let sequence = engine.show.sequence(1).unwrap();
        let reference = render::render(
            &engine.show,
            &Levels::new(),
            &sequence.state_at(0),
            &crate::effect::EffectOutput::default(),
            &Masters::new(),
            1.0,
        );
        assert_eq!(engine.preview_frame().unwrap(), reference);

        // A programmer edit changes the stage, not the inspected cue.
        engine.execute("Fixture 1 At 50").unwrap();
        assert_eq!(engine.preview_frame().unwrap().slot(1, 1), Some(255));
        let sheet = engine.execute("Sheet").unwrap().lines.join("\n");
        assert!(sheet.contains("Preview cue 1 in sequence 1"), "{sheet}");
        assert!(sheet.contains("1.001 255"), "{sheet}");
    }

    #[test]
    fn preview_of_a_missing_cue_names_the_sequence_and_cue() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        let error = engine.execute("Preview Cue 9").unwrap_err();
        assert!(error.message.contains("sequence 1"), "{error}");
        assert!(error.message.contains("cue 9"), "{error}");
        assert!(engine.desk.preview.is_none());
    }

    #[test]
    fn a_bare_preview_targets_the_next_cue_of_the_running_sequence() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 25").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Fixture 1 At 75").unwrap();
        engine.execute("Store Cue 2").unwrap();
        engine.execute("Clear All").unwrap();
        engine.execute("Go Sequence 1").unwrap();

        engine.execute("Preview").unwrap();
        assert_eq!(
            engine.desk.preview,
            Some(Preview {
                sequence: 1,
                cue: 2.0
            })
        );
        assert_eq!(engine.preview_frame().unwrap().slot(1, 1), Some(191));
    }

    #[test]
    fn deleting_or_moving_a_previewed_cue_keeps_the_preview_honest() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Preview Cue 1").unwrap();

        engine.execute("Cue 1 Move 4").unwrap();
        assert_eq!(
            engine.desk.preview,
            Some(Preview {
                sequence: 1,
                cue: 4.0
            }),
            "a preview follows a renumbered cue"
        );
        assert_eq!(engine.preview_frame().unwrap().slot(1, 1), Some(255));

        let outcome = engine.execute("Cue 4 Move 1").unwrap();
        assert!(!outcome.lines[0].contains("preview cleared"));
        let outcome = engine.execute("Delete Cue 1").unwrap();
        assert!(outcome.lines[0].contains("preview cleared"), "{outcome:?}");
        assert!(engine.desk.preview.is_none());
        assert!(engine.preview_frame().is_none());
    }

    #[test]
    fn status_names_the_preview_and_its_value_count() {
        let mut engine = engine();
        engine.execute("Fixture 1 At Full").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R 50").unwrap();
        engine.execute("Store Cue 1").unwrap();
        engine.execute("Preview Cue 1").unwrap();
        let status = engine.execute("Status").unwrap().lines.join("\n");
        assert!(
            status.contains("Preview: cue 1 in sequence 1 (2 values)"),
            "{status}"
        );
    }

    #[test]
    fn highlight_lifts_selected_fixtures_and_clears_on_clear() {
        let mut engine = two_fixture_engine();
        engine.execute("Fixture 2 At 50").unwrap();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Fixture 1 At ColorAdd_R 50").unwrap();
        engine.execute("Highlight").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(255), "fixture 1 is full");
        assert_eq!(engine.render().slot(1, 2), Some(128), "colour is untouched");
        assert_eq!(
            engine.render().slot(1, 5),
            Some(128),
            "fixture 2 is not selected"
        );

        let sheet = engine.execute("Sheet").unwrap().lines.join("\n");
        assert!(
            sheet.contains("Dimmer 20%"),
            "the listing keeps the programmed look: {sheet}"
        );
        engine.execute("Store Cue 1").unwrap();
        let cue = engine.show.sequence(1).unwrap().cue(1.0).unwrap();
        assert_eq!(
            cue.levels.get(1, "Dimmer"),
            Some(0.2),
            "highlight is not a programmer value and cannot be stored"
        );

        // Moving the selection while highlight is on must wake the output
        // thread even though no programmer value changed.
        let revision = engine.revision;
        engine.execute("Fixture 2").unwrap();
        assert!(engine.revision > revision);
        assert_eq!(
            engine.render().slot(1, 5),
            Some(255),
            "the new selection is full"
        );
        assert_eq!(engine.render().slot(1, 1), Some(51));

        engine.execute("Clear").unwrap();
        assert!(
            !engine.desk.highlight,
            "clearing the selection clears the aid"
        );
        assert_eq!(engine.render().slot(1, 1), Some(51));
    }

    #[test]
    fn highlight_needs_a_selection_and_blackout_wins() {
        let mut engine = engine();
        let error = engine.execute("Highlight").unwrap_err();
        assert!(error.message.contains("nothing selected"), "{error}");

        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Highlight").unwrap();
        assert_eq!(engine.render().slot(1, 1), Some(255));
        engine.execute("Blackout").unwrap();
        assert!(!engine.desk.highlight, "a blackout clears the aid");
        assert_eq!(engine.render().slot(1, 1), Some(0));
    }

    #[test]
    fn highlight_is_suppressed_while_blind() {
        let mut engine = engine();
        engine.execute("Fixture 1 At 20").unwrap();
        engine.execute("Highlight").unwrap();
        engine.execute("Blind").unwrap();
        assert_eq!(
            engine.render().slot(1, 1),
            Some(0),
            "a blind desk has nothing to light"
        );
        engine.execute("Blind Off").unwrap();
        assert_eq!(
            engine.render().slot(1, 1),
            Some(255),
            "highlight is still on"
        );
    }

    #[test]
    fn autosave_writes_a_snapshot_and_save_refreshes_it() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "openlight-{}-autosave.olshow.autosave",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let show_path =
            path.with_file_name(format!("openlight-{}-autosave.olshow", std::process::id()));
        let _ = std::fs::remove_file(&show_path);

        let mut engine = engine();
        let journal = journal_path("autosave-rev");
        engine.set_journal(Some(
            crate::journal::Journal::open(journal.clone()).unwrap(),
        ));
        engine.enable_autosave(path.clone(), Duration::from_secs(30));
        engine.execute("Fixture 1 At 60").unwrap();
        let start = engine.clock();
        engine.tick(start + Duration::from_secs(31));

        let snapshot = crate::journal::Snapshot::load(&path).unwrap().unwrap();
        let (snapshot_show, snapshot_desk, warnings) = snapshot.restore();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(snapshot_show, engine.show);
        assert_eq!(snapshot_desk.programmer.get(1, "Dimmer"), Some(0.6));

        engine
            .execute(&format!("Save \"{}\"", show_path.display()))
            .unwrap();
        let snapshot = crate::journal::Snapshot::load(&path).unwrap().unwrap();
        assert_eq!(
            snapshot.rev,
            Some(engine.journal().unwrap().rev()),
            "a snapshot records a journal revision, not a render count"
        );
        assert_eq!(snapshot.rev, Some(2), "the edit, then the save checkpoint");

        let lines = engine.execute("Status").unwrap().lines;
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Autosave: every 30s")),
            "status names the autosave: {lines:#?}"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&show_path);
        let _ = std::fs::remove_file(show_path.with_extension("oldesk"));
        let _ = std::fs::remove_file(&journal);
    }
}

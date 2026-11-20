//! Desk state: the handles, the programmer and where the playbacks are.
//!
//! A show is what an operator *programmed*; a desk is where the handles are and
//! what is happening now. Keeping them apart is what lets `Save` write a
//! document that does not contain a half-finished look, and lets a show travel
//! to another desk without dragging someone else's fader positions along.
//!
//! The desk file is disposable. It is written next to the show as
//! `<show>.oldesk`, it is validated by dropping anything that no longer exists
//! in the show (with a warning), and a missing or unreadable desk file is never
//! an error — a show without its desk starts clean.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::effect::{DEFAULT_BPM, MAX_BPM, MIN_BPM};
use crate::levels::Levels;
use crate::playback::format_cue_number;
use crate::show::{Show, MAX_RATE_MASTER};

pub const DESK_FORMAT: &str = "openlight-desk";
pub const DESK_VERSION: u32 = 1;

fn full() -> f64 {
    1.0
}

/// Every fader position on the desk. None of these is show data: a master is a
/// physical position, so a show loading must not move it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Masters {
    /// Scales every intensity at the output, 0..=1.
    #[serde(default = "full")]
    pub grand: f64,
    /// Scales effect speed, 0..=4 (100% is a factor of 1).
    #[serde(default = "full")]
    pub rate: f64,
    /// Group masters by group number, 0..=1. Missing means full.
    #[serde(default)]
    pub groups: BTreeMap<u16, f64>,
    /// Executor faders in desk version 1, where they lived here. Read once and
    /// moved to [`Desk::executors`]; never written again.
    #[serde(
        default,
        rename = "executors",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub(crate) legacy_executors: BTreeMap<u16, f64>,
}

impl Default for Masters {
    fn default() -> Self {
        Self::new()
    }
}

impl Masters {
    pub fn new() -> Self {
        Self {
            grand: 1.0,
            rate: 1.0,
            groups: BTreeMap::new(),
            legacy_executors: BTreeMap::new(),
        }
    }

    pub fn group(&self, number: u16) -> f64 {
        self.groups.get(&number).copied().unwrap_or(1.0)
    }

    pub fn set_group(&mut self, number: u16, level: f64) {
        set_level(&mut self.groups, number, level);
    }
}

/// One executor's handles: fader, flash and solo.
///
/// The show says what an executor plays and what its button does; this says
/// where the handles are. It never travels inside a show.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorDesk {
    /// 0..=1, the live position.
    #[serde(default = "full")]
    pub fader: f64,
    /// True while flash is held: the contribution is full whatever the fader
    /// says. Releasing restores the fader.
    #[serde(default)]
    pub flash: bool,
    /// True while solo is held: every other playback is silent.
    #[serde(default)]
    pub solo: bool,
}

impl Default for ExecutorDesk {
    fn default() -> Self {
        Self {
            fader: 1.0,
            flash: false,
            solo: false,
        }
    }
}

/// Store a fader position, dropping the entry when it is at full so a desk file
/// only names the faders someone actually moved.
fn set_level(faders: &mut BTreeMap<u16, f64>, number: u16, level: f64) {
    if (level - 1.0).abs() < f64::EPSILON {
        faders.remove(&number);
    } else {
        faders.insert(number, level);
    }
}

/// A blackout in progress: a master ramping from `from` to `level`.
///
/// Blackout is a render-stage master, never an edit, so lifting it restores the
/// look exactly as it was and leaves nothing in the show or the programmer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlackoutState {
    pub from: f64,
    pub level: f64,
    pub started: Instant,
    pub fade: Duration,
}

impl BlackoutState {
    /// The master at `now`: a pure function of time, so tests never sleep.
    pub fn master(&self, now: Instant) -> f64 {
        if self.fade.is_zero() {
            return self.level;
        }
        let elapsed = now.saturating_duration_since(self.started);
        let t = (elapsed.as_secs_f64() / self.fade.as_secs_f64()).clamp(0.0, 1.0);
        self.from + (self.level - self.from) * t
    }
}

/// Where one playback is: which cue, and whether it is held.
///
/// This is a *projection* of the live run, not the run itself. The desk records
/// which cue each target is on; the fade machinery stays in [`crate::playback`]
/// and is rebuilt on load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunState {
    /// The sequence the run belongs to, for a run started from the cue list.
    #[serde(default)]
    pub sequence: Option<u16>,
    /// The executor the run belongs to, for a run started from a fader.
    #[serde(default)]
    pub executor: Option<u16>,
    pub cue: f64,
    #[serde(default)]
    pub paused: bool,
}

impl RunState {
    pub fn target_label(&self) -> String {
        match (self.sequence, self.executor) {
            (_, Some(executor)) => format!("Executor {executor}"),
            (Some(sequence), None) => format!("Sequence {sequence}"),
            (None, None) => "Playback".to_string(),
        }
    }
}

/// A stored macro: a named list of statement text.
///
/// The text is kept exactly as entered so a macro can be listed, edited and
/// replayed through the same command path as typed input. Macros are desk
/// configuration, not show programming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Macro {
    pub name: String,
    #[serde(default)]
    pub statements: Vec<String>,
}

/// A cue the operator is inspecting. Not a saved intent: the engine resolves
/// the cue number on every render, so a preview follows edits to the show until
/// the cue it names is gone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preview {
    pub sequence: u16,
    pub cue: f64,
}

impl Preview {
    pub fn label(&self) -> String {
        format!(
            "cue {} in sequence {}",
            format_cue_number(self.cue),
            self.sequence
        )
    }
}

/// Everything the desk owns. Survives a save; never travels inside a show.
#[derive(Debug, Clone, PartialEq)]
pub struct Desk {
    /// Fixtures the operator has selected, in the order they were addressed.
    pub selection: Vec<u16>,
    /// The programmer: un-stored values that win over playback.
    pub programmer: Levels,
    /// True while the programmer is kept off the stage.
    pub blind: bool,
    /// True while selected fixtures are held at full intensity as an output
    /// aid. Never a programmer value, so it cannot be stored into a cue.
    pub highlight: bool,
    /// The cue `Sheet` is inspecting. Transient: never written to the desk
    /// file, because a preview is a view, not a saved intent.
    pub preview: Option<Preview>,
    /// Active world: limits selection and programming, never output.
    pub world: Option<u16>,
    /// Active filter: limits the features commands may touch.
    pub filter: Option<u16>,
    pub masters: Masters,
    /// Executor handles by executor number. Missing means a fader at full and
    /// no button held.
    pub executors: BTreeMap<u16, ExecutorDesk>,
    /// A blackout in progress. Saved as its level, so a recovered desk comes
    /// back dark rather than unexpectedly lit.
    pub blackout: Option<BlackoutState>,
    /// `None` is unlocked; `Some(code)` refuses every mutating command. This is
    /// a lock, not authentication: the code is plaintext in the desk file.
    pub lock: Option<String>,
    /// Playback positions, written on save and applied on load.
    pub runs: Vec<RunState>,
    /// Stored macros by number. Statement text, not compiled commands.
    pub macros: BTreeMap<u16, Macro>,
    /// Desk tempo in beats per minute. Musical effects follow this live.
    pub tempo: f64,
    /// The last few `Tap Tempo` instants. Session state: never written to the
    /// desk file.
    pub taps: VecDeque<Instant>,
}

impl Default for Desk {
    fn default() -> Self {
        Self {
            selection: Vec::new(),
            programmer: Levels::default(),
            blind: false,
            highlight: false,
            preview: None,
            world: None,
            filter: None,
            masters: Masters::default(),
            executors: BTreeMap::new(),
            blackout: None,
            lock: None,
            runs: Vec::new(),
            macros: BTreeMap::new(),
            tempo: DEFAULT_BPM,
            taps: VecDeque::new(),
        }
    }
}

impl Desk {
    pub fn new() -> Self {
        Self {
            masters: Masters::new(),
            ..Self::default()
        }
    }

    /// Drop everything that refers to the show currently loaded.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// The blackout master: 1 when there is no blackout.
    pub fn blackout_master(&self, now: Instant) -> f64 {
        self.blackout.map(|state| state.master(now)).unwrap_or(1.0)
    }

    /// Average the last taps into a tempo. `None` means there are not enough
    /// taps yet (or they landed on the same instant). A gap longer than two
    /// seconds starts a new count-in instead of averaging a pause.
    pub fn tap(&mut self, now: Instant) -> Option<f64> {
        if let Some(last) = self.taps.back() {
            if now.saturating_duration_since(*last) > Duration::from_secs(2) {
                self.taps.clear();
            }
        }
        self.taps.push_back(now);
        while self.taps.len() > 5 {
            self.taps.pop_front();
        }
        if self.taps.len() < 2 {
            return None;
        }
        let mut intervals = Vec::with_capacity(self.taps.len() - 1);
        let mut previous = *self.taps.front()?;
        for tap in self.taps.iter().skip(1) {
            intervals.push(tap.saturating_duration_since(previous).as_secs_f64());
            previous = *tap;
        }
        let average = intervals.iter().sum::<f64>() / intervals.len() as f64;
        if average <= 0.0 {
            return None;
        }
        self.tempo = (60.0 / average).clamp(MIN_BPM, MAX_BPM);
        Some(self.tempo)
    }

    pub fn executor_state(&self, number: u16) -> ExecutorDesk {
        self.executors.get(&number).copied().unwrap_or_default()
    }

    /// How loud one executor is, the single expression rendering uses.
    ///
    /// Flash overrides the fader, and a solo silences everyone else at the
    /// merge step rather than changing any run.
    pub fn executor_master(&self, number: u16) -> f64 {
        let state = self.executor_state(number);
        let base = if state.flash { 1.0 } else { state.fader };
        match self.soloed() {
            Some(soloed) if soloed != number => 0.0,
            _ => base,
        }
    }

    /// The first soloed executor. `Status` names it, so an empty stage is
    /// diagnosable in one command.
    pub fn soloed(&self) -> Option<u16> {
        self.executors
            .iter()
            .find(|(_, state)| state.solo)
            .map(|(number, _)| *number)
    }

    /// Sequences are not executors, so a solo takes them off the stage too.
    pub fn sequence_master(&self) -> f64 {
        if self.soloed().is_some() {
            0.0
        } else {
            1.0
        }
    }

    /// Move an executor fader. A handle left at its default is dropped, so a
    /// desk file only names the handles someone touched.
    pub fn set_executor_fader(&mut self, number: u16, level: f64) {
        self.executors.entry(number).or_default().fader = level.clamp(0.0, 1.0);
        self.tidy_executor(number);
    }

    pub fn set_executor_flash(&mut self, number: u16, flash: bool) {
        self.executors.entry(number).or_default().flash = flash;
        self.tidy_executor(number);
    }

    pub fn set_executor_solo(&mut self, number: u16, solo: bool) {
        self.executors.entry(number).or_default().solo = solo;
        self.tidy_executor(number);
    }

    fn tidy_executor(&mut self, number: u16) {
        if self.executor_state(number) == ExecutorDesk::default() {
            self.executors.remove(&number);
        }
    }

    /// True when the stage is fully dark because of a blackout.
    pub fn is_blacked_out(&self, now: Instant) -> bool {
        self.blackout.is_some() && self.blackout_master(now) <= 1e-9
    }

    /// Start or finish a blackout. `level` is 0 to black out, 1 to lift it.
    pub fn set_blackout(&mut self, level: f64, fade: Duration, now: Instant) {
        let from = self.blackout_master(now);
        self.blackout = Some(BlackoutState {
            from,
            level,
            started: now,
            fade,
        });
    }

    /// The desk file that belongs to a show file: `look.olshow` → `look.oldesk`.
    pub fn path_for(show_path: &Path) -> PathBuf {
        show_path.with_extension("oldesk")
    }

    /// Remove anything the show no longer has, reporting each drop.
    ///
    /// A desk file is disposable, so a stale number is a warning, never an
    /// error: the alternative is a show that will not open because a fixture
    /// was unpatched on another machine.
    pub fn sanitise(&mut self, show: &Show) -> Vec<String> {
        let mut warnings = Vec::new();

        let before = self.selection.len();
        self.selection
            .retain(|number| show.fixture(*number).is_some());
        self.selection.dedup();
        for _ in self.selection.len()..before {
            warnings.push("desk: a selected fixture is no longer patched; dropped".into());
        }

        let mut stale = Vec::new();
        for (fixture, attributes) in self.programmer.fixtures() {
            let Some(patched) = show.fixture(fixture) else {
                stale.push((fixture, None));
                continue;
            };
            let Ok((_, mode)) = show.mode_for(patched) else {
                stale.push((fixture, None));
                continue;
            };
            for attribute in attributes.keys() {
                if mode.channel_for(attribute).is_none() {
                    stale.push((fixture, Some(attribute.clone())));
                }
            }
        }
        for (fixture, attribute) in stale {
            match attribute {
                Some(attribute) => {
                    self.programmer.remove(fixture, &attribute);
                    warnings.push(format!(
                        "desk: fixture {fixture} has no `{attribute}`; programmer value dropped"
                    ));
                }
                None => {
                    self.programmer.remove_fixture(fixture);
                    warnings.push(format!(
                        "desk: fixture {fixture} is not patched; programmer values dropped"
                    ));
                }
            }
        }

        if let Some(preview) = self.preview {
            let live = show
                .sequence(preview.sequence)
                .and_then(|sequence| sequence.cue_index(preview.cue))
                .is_some();
            if !live {
                self.preview = None;
                warnings.push(format!(
                    "desk: preview of {} no longer exists; cleared",
                    preview.label()
                ));
            }
        }

        if let Some(number) = self.world {
            if show.world(number).is_none() {
                self.world = None;
                warnings.push(format!(
                    "desk: world {number} is not in the show; deactivated"
                ));
            }
        }
        if let Some(number) = self.filter {
            if show.filter(number).is_none() {
                self.filter = None;
                warnings.push(format!(
                    "desk: filter {number} is not in the show; deactivated"
                ));
            }
        }

        self.masters.sanitise(show, &mut warnings);
        if !self.tempo.is_finite() || !(MIN_BPM..=MAX_BPM).contains(&self.tempo) {
            warnings.push(format!(
                "desk: tempo {} BPM is outside {MIN_BPM}..={MAX_BPM}; reset to {DEFAULT_BPM}",
                self.tempo
            ));
            self.tempo = DEFAULT_BPM;
        }

        let executors: BTreeSet<u16> = show
            .executors
            .iter()
            .map(|executor| executor.number)
            .collect();
        self.executors.retain(|number, state| {
            let live = executors.contains(number)
                && state.fader.is_finite()
                && (0.0..=1.0).contains(&state.fader);
            if !live {
                warnings.push(format!(
                    "desk: executor {number} handle dropped (executor not in the show)"
                ));
            }
            live
        });

        let before = self.runs.len();
        self.runs.retain(|run| run_is_live(run, show));
        for _ in self.runs.len()..before {
            warnings.push("desk: a playback position no longer exists; dropped".into());
        }

        let macros = self.macros.len();
        self.macros.retain(|number, stored| {
            *number > 0
                && !stored.name.trim().is_empty()
                && !stored.statements.is_empty()
                && stored.statements.iter().all(|line| !line.trim().is_empty())
        });
        for _ in self.macros.len()..macros {
            warnings.push("desk: an invalid macro was dropped".into());
        }

        warnings
    }

    /// Read the desk file beside `show_path`, if there is one.
    pub fn load(path: &Path, show: &Show) -> Result<Option<(Desk, Vec<String>)>, DeskError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(DeskError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let file: DeskFile = serde_json::from_str(&text).map_err(|source| DeskError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        Self::check(&file, path)?;
        Ok(Some(Self::from_file(file, show)))
    }

    fn check(file: &DeskFile, path: &Path) -> Result<(), DeskError> {
        if file.format != DESK_FORMAT {
            return Err(DeskError::WrongFormat {
                path: path.to_path_buf(),
                found: file.format.clone(),
            });
        }
        if file.version > DESK_VERSION {
            return Err(DeskError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: file.version,
                supported: DESK_VERSION,
            });
        }
        Ok(())
    }

    /// Turn a desk file into live desk state, dropping what the show lacks.
    pub(crate) fn from_file(file: DeskFile, show: &Show) -> (Desk, Vec<String>) {
        let mut masters = file.masters;
        let legacy = std::mem::take(&mut masters.legacy_executors);
        let mut executors = file.executors;
        for (number, fader) in legacy {
            executors.entry(number).or_default().fader = fader;
        }
        let mut desk = Desk {
            selection: file.selection,
            programmer: file.programmer,
            blind: file.blind,
            highlight: file.highlight,
            // A preview is a view, not an intent: it never survives a reload.
            preview: None,
            world: file.world,
            filter: file.filter,
            masters,
            executors,
            // A saved blackout comes back at its level, not mid-ramp.
            blackout: file.blackout.map(|saved| BlackoutState {
                from: saved.level,
                level: saved.level,
                started: Instant::now(),
                fade: Duration::ZERO,
            }),
            lock: file.lock,
            runs: file.runs,
            macros: file.macros,
            tempo: file.tempo,
            taps: VecDeque::new(),
        };
        let mut warnings = desk.sanitise(show);
        if !file.show.is_empty() && file.show != show.name {
            warnings.push(format!(
                "desk: written for show `{}`, loading into `{}`",
                file.show, show.name
            ));
        }
        (desk, warnings)
    }

    /// The persistable form of this desk.
    pub(crate) fn to_file(&self, show: &Show) -> DeskFile {
        DeskFile {
            format: DESK_FORMAT.to_string(),
            version: DESK_VERSION,
            show: show.name.clone(),
            selection: self.selection.clone(),
            programmer: self.programmer.clone(),
            blind: self.blind,
            highlight: self.highlight,
            world: self.world,
            filter: self.filter,
            masters: self.masters.clone(),
            executors: self.executors.clone(),
            blackout: self.blackout.map(|state| SavedBlackout {
                level: state.level,
                fade: state.fade.as_secs_f64(),
            }),
            lock: self.lock.clone(),
            runs: self.runs.clone(),
            macros: self.macros.clone(),
            tempo: self.tempo,
        }
    }

    /// Write this desk beside its show.
    pub fn save(&self, path: &Path, show: &Show) -> Result<(), DeskError> {
        let file = self.to_file(show);
        let json = serde_json::to_string_pretty(&file).map_err(|source| DeskError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        crate::store::write_atomic(path, json.as_bytes()).map_err(|source| DeskError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

impl Masters {
    fn sanitise(&mut self, show: &Show, warnings: &mut Vec<String>) {
        let check = |label: &str, value: f64, max: f64, warnings: &mut Vec<String>| {
            if !value.is_finite() || !(0.0..=max).contains(&value) {
                warnings.push(format!(
                    "desk: {label} {value} is outside 0..={max}; reset to full"
                ));
                Some(1.0)
            } else {
                None
            }
        };
        if let Some(value) = check("grand master", self.grand, 1.0, warnings) {
            self.grand = value;
        }
        if let Some(value) = check("rate master", self.rate, MAX_RATE_MASTER, warnings) {
            self.rate = value;
        }

        let groups: BTreeSet<u16> = show.groups.iter().map(|group| group.number).collect();
        self.groups.retain(|number, level| {
            let live = groups.contains(number) && level.is_finite() && (0.0..=1.0).contains(level);
            if !live {
                warnings.push(format!(
                    "desk: group {number} master dropped (group not in the show)"
                ));
            }
            live
        });
    }
}

fn run_is_live(run: &RunState, show: &Show) -> bool {
    let sequence = match (run.executor, run.sequence) {
        (Some(executor), _) => show.executor(executor).map(|executor| executor.sequence),
        (None, Some(sequence)) => Some(sequence),
        (None, None) => None,
    };
    let Some(sequence) = sequence.and_then(|number| show.sequence(number)) else {
        return false;
    };
    sequence.cue_index(run.cue).is_some()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeskFile {
    format: String,
    version: u32,
    #[serde(default)]
    show: String,
    #[serde(default)]
    selection: Vec<u16>,
    #[serde(default)]
    programmer: Levels,
    #[serde(default)]
    blind: bool,
    #[serde(default)]
    highlight: bool,
    #[serde(default)]
    world: Option<u16>,
    #[serde(default)]
    filter: Option<u16>,
    #[serde(default)]
    masters: Masters,
    #[serde(default)]
    executors: BTreeMap<u16, ExecutorDesk>,
    #[serde(default)]
    blackout: Option<SavedBlackout>,
    #[serde(default)]
    lock: Option<String>,
    #[serde(default)]
    runs: Vec<RunState>,
    #[serde(default)]
    macros: BTreeMap<u16, Macro>,
    #[serde(default = "default_tempo")]
    tempo: f64,
}

fn default_tempo() -> f64 {
    DEFAULT_BPM
}

/// The persistable part of a blackout: where it is, not when it started.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedBlackout {
    level: f64,
    #[serde(default)]
    fade: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum DeskError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{}: not an OpenLight desk (format `{found}`)", path.display())]
    WrongFormat { path: PathBuf, found: String },
    #[error(
        "{}: desk version {found} is not supported (this build reads 1..={supported})",
        path.display()
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::demo_show;
    use std::fs;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "openlight-desk-{}-{name}.oldesk.json",
            std::process::id()
        ))
    }

    fn desk_with_values() -> Desk {
        let mut desk = Desk::new();
        desk.selection = vec![1];
        desk.programmer.set(1, "Dimmer", 0.5);
        desk.masters.grand = 0.75;
        desk.tempo = 132.0;
        desk
    }

    #[test]
    fn taps_average_to_a_tempo_and_a_long_gap_restarts() {
        let mut desk = Desk::new();
        let start = Instant::now();
        assert_eq!(desk.tap(start), None, "one tap is a count-in");
        assert_eq!(desk.tap(start + Duration::from_millis(500)), Some(120.0));
        assert_eq!(desk.tap(start + Duration::from_secs(1)), Some(120.0));
        assert_eq!(
            desk.tap(start + Duration::from_secs(4)),
            None,
            "a 3 s gap starts a new count-in"
        );
        assert_eq!(desk.taps.len(), 1);
    }

    #[test]
    fn a_desk_file_without_a_tempo_loads_at_the_default() {
        let show = demo_show();
        let desk = desk_with_values();
        let path = temp_path("old-tempo");
        desk.save(&path, &show).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("tempo");
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();
        let (loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert_eq!(loaded.tempo, DEFAULT_BPM);
        assert!(warnings.is_empty(), "{warnings:?}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn round_trips() {
        let show = demo_show();
        let desk = desk_with_values();
        let path = temp_path("roundtrip");
        desk.save(&path, &show).unwrap();
        let (loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert_eq!(loaded, desk);
        assert!(warnings.is_empty(), "{warnings:?}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn macros_round_trip_and_invalid_ones_are_dropped() {
        let show = demo_show();
        let mut desk = Desk::new();
        desk.macros.insert(
            1,
            Macro {
                name: "Blackout cue".into(),
                statements: vec!["Blackout".into(), "Wait 2".into(), "Blackout Off".into()],
            },
        );
        let path = temp_path("macros");
        desk.save(&path, &show).unwrap();
        let (mut loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.macros, desk.macros);

        loaded.macros.insert(
            2,
            Macro {
                name: String::new(),
                statements: Vec::new(),
            },
        );
        let warnings = loaded.sanitise(&show);
        assert_eq!(loaded.macros.len(), 1);
        assert!(
            warnings.iter().any(|warning| warning.contains("macro")),
            "{warnings:?}"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_missing_desk_file_is_not_an_error() {
        let show = demo_show();
        let path = temp_path("missing");
        assert_eq!(Desk::load(&path, &show).unwrap(), None);
    }

    #[test]
    fn a_show_name_mismatch_warns_and_still_loads() {
        let mut show = demo_show();
        let desk = desk_with_values();
        let path = temp_path("mismatch");
        desk.save(&path, &show).unwrap();
        show.name = "Another Show".into();
        let (loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert_eq!(loaded.masters.grand, 0.75);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("Another Show")),
            "{warnings:?}"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn dangling_fixtures_and_targets_are_dropped_with_a_warning() {
        let show = demo_show();
        let mut desk = desk_with_values();
        desk.selection.push(99);
        desk.programmer.set(99, "Dimmer", 0.4);
        desk.programmer.set(1, "NotAnAttribute", 0.4);
        desk.world = Some(7);
        desk.filter = Some(7);
        desk.masters.set_group(3, 0.5);
        desk.set_executor_fader(4, 0.5);
        desk.runs.push(RunState {
            sequence: Some(9),
            executor: None,
            cue: 1.0,
            paused: false,
        });

        let path = temp_path("dangling");
        desk.save(&path, &show).unwrap();
        let (loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert_eq!(loaded.selection, vec![1]);
        assert_eq!(loaded.programmer.get(99, "Dimmer"), None);
        assert_eq!(loaded.programmer.get(1, "Dimmer"), Some(0.5));
        assert_eq!(loaded.programmer.get(1, "NotAnAttribute"), None);
        assert_eq!(loaded.world, None);
        assert_eq!(loaded.filter, None);
        assert!(loaded.masters.groups.is_empty());
        assert!(loaded.executors.is_empty());
        assert!(loaded.runs.is_empty());
        assert!(warnings.len() >= 6, "{warnings:#?}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn blind_and_highlight_round_trip_but_preview_does_not() {
        let show = demo_show();
        let mut desk = Desk::new();
        desk.blind = true;
        desk.highlight = true;
        desk.preview = Some(Preview {
            sequence: 1,
            cue: 2.0,
        });

        let path = temp_path("modes");
        desk.save(&path, &show).unwrap();
        let (loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(loaded.blind, "blind is saved desk state");
        assert!(loaded.highlight, "highlight is saved desk state");
        assert_eq!(loaded.preview, None, "a preview is a transient view");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_dangling_preview_is_cleared_with_a_warning() {
        let show = demo_show();
        let mut desk = Desk::new();
        desk.preview = Some(Preview {
            sequence: 9,
            cue: 4.0,
        });
        let warnings = desk.sanitise(&show);
        assert_eq!(desk.preview, None);
        assert!(
            warnings.iter().any(|warning| warning.contains("preview")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_desk_for_another_show_is_rejected_by_format_or_version() {
        let show = demo_show();
        let path = temp_path("future");
        fs::write(
            &path,
            r#"{"format":"openlight-desk","version":9,"show":"x"}"#,
        )
        .unwrap();
        assert!(matches!(
            Desk::load(&path, &show),
            Err(DeskError::UnsupportedVersion { found: 9, .. })
        ));
        fs::write(&path, r#"{"format":"other","version":1}"#).unwrap();
        assert!(matches!(
            Desk::load(&path, &show),
            Err(DeskError::WrongFormat { .. })
        ));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_desk_path_sits_beside_the_show() {
        assert_eq!(
            Desk::path_for(Path::new("shows/look.olshow")),
            PathBuf::from("shows/look.oldesk")
        );
    }

    #[test]
    fn blackout_and_lock_round_trip() {
        let show = demo_show();
        let mut desk = Desk::new();
        let now = Instant::now();
        desk.set_blackout(0.0, Duration::from_secs(2), now);
        desk.lock = Some("4711".into());

        let path = temp_path("blackout");
        desk.save(&path, &show).unwrap();
        let (loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(loaded.is_blacked_out(Instant::now()));
        assert_eq!(loaded.lock.as_deref(), Some("4711"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn executor_master_combines_fader_flash_and_solo() {
        let mut desk = Desk::new();
        assert_eq!(
            desk.executor_master(1),
            1.0,
            "an untouched executor is full"
        );
        desk.set_executor_fader(1, 0.4);
        assert_eq!(desk.executor_master(1), 0.4);

        desk.set_executor_flash(1, true);
        assert_eq!(desk.executor_master(1), 1.0, "flash ignores the fader");
        desk.set_executor_flash(1, false);
        assert_eq!(desk.executor_master(1), 0.4, "releasing restores the fader");

        desk.set_executor_fader(2, 0.8);
        desk.set_executor_solo(2, true);
        assert_eq!(desk.soloed(), Some(2));
        assert_eq!(
            desk.executor_master(2),
            0.8,
            "the soloed executor keeps its level"
        );
        assert_eq!(
            desk.executor_master(1),
            0.0,
            "every other executor is silent"
        );
        assert_eq!(desk.sequence_master(), 0.0);
        assert_eq!(desk.executor_master(3), 0.0, "even an untouched one");

        desk.set_executor_solo(2, false);
        assert_eq!(desk.soloed(), None);
        assert_eq!(desk.executor_master(1), 0.4);
        assert_eq!(desk.sequence_master(), 1.0);
    }

    #[test]
    fn executor_handles_round_trip() {
        let mut show = demo_show();
        show.executors.push(crate::playback::Executor {
            number: 3,
            sequence: 1,
            name: "Main".into(),
            mode: crate::playback::ExecutorMode::Fader,
        });
        let mut desk = Desk::new();
        desk.set_executor_fader(3, 0.25);
        desk.set_executor_flash(3, true);
        desk.set_executor_solo(3, true);

        let path = temp_path("executors");
        desk.save(&path, &show).unwrap();
        let (loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.executor_state(3).fader, 0.25);
        assert!(loaded.executor_state(3).flash);
        assert!(loaded.executor_state(3).solo);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_version_one_desk_hands_its_executor_faders_over() {
        let mut show = demo_show();
        show.executors.push(crate::playback::Executor {
            number: 2,
            sequence: 1,
            name: "Main".into(),
            mode: crate::playback::ExecutorMode::Go,
        });
        let path = temp_path("legacy-executors");
        fs::write(
            &path,
            format!(
                r#"{{"format":"openlight-desk","version":1,"show":"{}",
                   "masters":{{"grand":1.0,"rate":1.0,"executors":{{"2":0.4}}}}}}"#,
                show.name
            ),
        )
        .unwrap();
        let (loaded, warnings) = Desk::load(&path, &show).unwrap().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.executor_state(2).fader, 0.4);
        assert!(
            loaded.masters.legacy_executors.is_empty(),
            "the old field is not carried forward"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_blackout_fades_from_where_it_was() {
        let mut desk = Desk::new();
        let start = Instant::now();
        desk.set_blackout(0.0, Duration::from_secs(2), start);
        assert_eq!(desk.blackout_master(start), 1.0);
        assert!((desk.blackout_master(start + Duration::from_secs(1)) - 0.5).abs() < 1e-9);
        assert_eq!(desk.blackout_master(start + Duration::from_secs(2)), 0.0);

        // Lifting mid-fade starts from the level it reached, not from 0.
        desk.set_blackout(1.0, Duration::from_secs(2), start + Duration::from_secs(1));
        assert!((desk.blackout_master(start + Duration::from_secs(2)) - 0.75).abs() < 1e-9);
    }
}

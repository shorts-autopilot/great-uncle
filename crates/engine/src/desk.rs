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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::levels::Levels;
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
    /// Executor faders by executor number, 0..=1. Missing means full.
    #[serde(default)]
    pub executors: BTreeMap<u16, f64>,
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
            executors: BTreeMap::new(),
        }
    }

    pub fn group(&self, number: u16) -> f64 {
        self.groups.get(&number).copied().unwrap_or(1.0)
    }

    pub fn executor(&self, number: u16) -> f64 {
        self.executors.get(&number).copied().unwrap_or(1.0)
    }

    pub fn set_group(&mut self, number: u16, level: f64) {
        set_level(&mut self.groups, number, level);
    }

    pub fn set_executor(&mut self, number: u16, level: f64) {
        set_level(&mut self.executors, number, level);
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

/// Everything the desk owns. Survives a save; never travels inside a show.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Desk {
    /// Fixtures the operator has selected, in the order they were addressed.
    pub selection: Vec<u16>,
    /// The programmer: un-stored values that win over playback.
    pub programmer: Levels,
    /// Active world: limits selection and programming, never output.
    pub world: Option<u16>,
    /// Active filter: limits the features commands may touch.
    pub filter: Option<u16>,
    pub masters: Masters,
    /// A blackout in progress. Saved as its level, so a recovered desk comes
    /// back dark rather than unexpectedly lit.
    pub blackout: Option<BlackoutState>,
    /// `None` is unlocked; `Some(code)` refuses every mutating command. This is
    /// a lock, not authentication: the code is plaintext in the desk file.
    pub lock: Option<String>,
    /// Playback positions, written on save and applied on load.
    pub runs: Vec<RunState>,
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

        let before = self.runs.len();
        self.runs.retain(|run| run_is_live(run, show));
        for _ in self.runs.len()..before {
            warnings.push("desk: a playback position no longer exists; dropped".into());
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
        let mut desk = Desk {
            selection: file.selection,
            programmer: file.programmer,
            world: file.world,
            filter: file.filter,
            masters: file.masters,
            // A saved blackout comes back at its level, not mid-ramp.
            blackout: file.blackout.map(|saved| BlackoutState {
                from: saved.level,
                level: saved.level,
                started: Instant::now(),
                fade: Duration::ZERO,
            }),
            lock: file.lock,
            runs: file.runs,
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
            world: self.world,
            filter: self.filter,
            masters: self.masters.clone(),
            blackout: self.blackout.map(|state| SavedBlackout {
                level: state.level,
                fade: state.fade.as_secs_f64(),
            }),
            lock: self.lock.clone(),
            runs: self.runs.clone(),
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

        let executors: BTreeSet<u16> = show
            .executors
            .iter()
            .map(|executor| executor.number)
            .collect();
        self.executors.retain(|number, level| {
            let live =
                executors.contains(number) && level.is_finite() && (0.0..=1.0).contains(level);
            if !live {
                warnings.push(format!(
                    "desk: executor {number} fader dropped (executor not in the show)"
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
    world: Option<u16>,
    #[serde(default)]
    filter: Option<u16>,
    #[serde(default)]
    masters: Masters,
    #[serde(default)]
    blackout: Option<SavedBlackout>,
    #[serde(default)]
    lock: Option<String>,
    #[serde(default)]
    runs: Vec<RunState>,
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
        desk
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
        desk.masters.set_executor(4, 0.5);
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
        assert!(loaded.masters.executors.is_empty());
        assert!(loaded.runs.is_empty());
        assert!(warnings.len() >= 6, "{warnings:#?}");
        let _ = fs::remove_file(&path);
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

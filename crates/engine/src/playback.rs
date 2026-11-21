//! Cues, sequences, executors and the running playback.
//!
//! Cues store **tracked deltas**: a cue holds only the values that differ from
//! the state the previous cues leave behind. The state of cue *n* is therefore
//! the accumulation of cues 0..=n, and a fade interpolates from the state
//! before the cue to the state of the cue. Editing an earlier cue changes what
//! later cues inherit, exactly like tracking on a console.
//!
//! Playback is time-driven and lives outside the show file: cue numbers, fade
//! and delay times are saved, while "which cue is running" is not.
//!
//! A cue may also start itself: `Follow` waits for the previous fade to settle,
//! `Time` counts from the previous cue's start. Both are resolved by
//! [`Playback::due`], a pure function of the show, the runs and an instant, so
//! the chain can be tested without a clock.
//!
//! A cue can stop tracking (`blocked`), which turns its stored levels into a
//! complete state instead of a delta, and it can shield an attribute at a fixed
//! level so an edit upstream does not move it. Structural edits (move, copy,
//! delete, block) go through [`Sequence::rebase`], which rewrites the deltas so
//! every cue's absolute look is unchanged.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::desk::Desk;
use crate::levels::{is_intensity, Levels};
use crate::show::Show;

/// Longest fade or delay accepted, in seconds (24 hours).
pub const MAX_TIME: f64 = 86_400.0;
/// Largest cue number accepted, so a typo cannot produce unreadable output.
pub const MAX_CUE_NUMBER: f64 = 1_000_000.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sequence {
    pub number: u16,
    #[serde(default)]
    pub name: String,
    /// Kept sorted by cue number.
    #[serde(default)]
    pub cues: Vec<Cue>,
}

/// How a cue starts.
///
/// An enum, not a string, so an unknown trigger is a deserialization error
/// rather than a cue that silently never fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    /// Waits for `Go`.
    #[default]
    Go,
    /// Starts `trigger_time` after the previous fade settles.
    Follow,
    /// Starts `trigger_time` after the previous cue started.
    Time,
}

impl Trigger {
    pub fn label(self) -> &'static str {
        match self {
            Trigger::Go => "Go",
            Trigger::Follow => "Follow",
            Trigger::Time => "Time",
        }
    }
}

/// `Go`, `Follow 1.5s` or `Time 2.0s`: one expression for every listing.
pub fn format_trigger(trigger: Trigger, time: f64) -> String {
    match trigger {
        Trigger::Go => "Go".to_string(),
        Trigger::Follow => format!("Follow {time:.1}s"),
        Trigger::Time => format!("Time {time:.1}s"),
    }
}

/// The shape of one fade lane.
///
/// Quadratic rather than cubic because it is cheap, monotone and visibly
/// different at desk scale. `Snap` is an instant change at the lane's start;
/// its fade time is kept for the record but not evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Curve {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Snap,
}

impl Curve {
    pub fn label(self) -> &'static str {
        match self {
            Curve::Linear => "Linear",
            Curve::EaseIn => "EaseIn",
            Curve::EaseOut => "EaseOut",
            Curve::EaseInOut => "EaseInOut",
            Curve::Snap => "Snap",
        }
    }

    /// Map progress through a fade (0..=1) to the fraction of the distance
    /// covered. Total on its whole range, so no input can produce a NaN.
    pub fn apply(self, t: f64) -> f64 {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            1.0
        };
        match self {
            Curve::Linear => t,
            Curve::EaseIn => t * t,
            Curve::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Curve::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - 2.0 * (1.0 - t) * (1.0 - t)
                }
            }
            Curve::Snap => {
                if t > 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

/// One attribute's override of a cue's fade, delay and curve.
///
/// `Option` rather than a sentinel means "follow the cue" is representable, so
/// a later cue-fade edit moves every attribute that did not override it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AttributeTiming {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<f64>,
    #[serde(default)]
    pub curve: Curve,
}

/// The timing a cue stores for one attribute, matched without regard to case:
/// the programmer and a fixture mode may spell the same attribute differently.
pub fn timing_for<'a>(cue: &'a Cue, attribute: &str) -> Option<&'a AttributeTiming> {
    cue.timing.get(attribute).or_else(|| {
        cue.timing
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(attribute))
            .map(|(_, timing)| timing)
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cue {
    /// Cue numbers may be fractional (`1.5` inserts a cue between 1 and 2).
    pub number: f64,
    #[serde(default)]
    pub name: String,
    /// Fade time in seconds.
    #[serde(default)]
    pub fade: f64,
    /// Delay before the fade starts, in seconds.
    #[serde(default)]
    pub delay: f64,
    /// Move in black: fixtures that are dark when the cue starts jump to its
    /// non-intensity values instead of fading them, so colour and position are
    /// already in place when the dimmer rises.
    #[serde(default)]
    pub mib: bool,
    /// What starts this cue.
    #[serde(default)]
    pub trigger: Trigger,
    /// Seconds for `Follow` and `Time`. Kept (and ignored) on a `Go` cue, so
    /// switching a trigger back and forth does not lose the number.
    #[serde(default)]
    pub trigger_time: f64,
    /// Per-attribute timing overrides, keyed by attribute name. An entry carries
    /// only what it overrides; a missing field follows the cue's own fade or
    /// delay.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub timing: BTreeMap<String, AttributeTiming>,
    /// Seconds each consecutive fixture waits before it starts, in the order
    /// the cue's target state lists the fixtures.
    #[serde(default)]
    pub stagger: f64,
    /// Stop tracking here: `levels` is the complete state, not a delta.
    #[serde(default)]
    pub blocked: bool,
    /// Attributes asserted at a fixed value here: an upstream edit does not
    /// change them. The value is stored rather than just a flag, because a flag
    /// cannot survive the very upstream edit it is meant to block.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shield: BTreeMap<String, f64>,
    /// Tracked values: only what changes relative to the previous cue.
    #[serde(default)]
    pub levels: Levels,
}

/// What the button of an executor does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExecutorMode {
    /// The button starts the next cue; the fader is a master.
    #[default]
    Go,
    /// The fader crossfades the cue by hand.
    Fader,
    /// The button runs the list at full while held.
    Flash,
    /// The button removes every other playback while held.
    Solo,
}

impl ExecutorMode {
    pub fn label(self) -> &'static str {
        match self {
            ExecutorMode::Go => "Go",
            ExecutorMode::Fader => "Fader",
            ExecutorMode::Flash => "Flash",
            ExecutorMode::Solo => "Solo",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Executor {
    /// The page this executor lives on. Page one is implicit, so a show with
    /// no pages reads exactly like a v12 document.
    #[serde(default = "page_one", skip_serializing_if = "is_page_one")]
    pub page: u16,
    pub number: u16,
    pub sequence: u16,
    #[serde(default)]
    pub name: String,
    /// What the button does. The fader position itself is desk state.
    #[serde(default)]
    pub mode: ExecutorMode,
}

impl Sequence {
    pub fn new(number: u16) -> Self {
        Self {
            number,
            name: format!("Sequence {number}"),
            cues: Vec::new(),
        }
    }

    pub fn cue_index(&self, number: f64) -> Option<usize> {
        self.cues
            .iter()
            .position(|cue| (cue.number - number).abs() < 1e-9)
    }

    pub fn cue(&self, number: f64) -> Option<&Cue> {
        self.cue_index(number).map(|index| &self.cues[index])
    }

    pub fn cue_mut(&mut self, number: f64) -> Option<&mut Cue> {
        let index = self.cue_index(number)?;
        self.cues.get_mut(index)
    }

    /// Where a cue with this number belongs, keeping the list sorted.
    pub fn insertion_index(&self, number: f64) -> usize {
        self.cues
            .iter()
            .position(|cue| cue.number > number)
            .unwrap_or(self.cues.len())
    }

    /// Accumulated state after cue `index` (all cues when `index` is past the end).
    pub fn state_at(&self, index: usize) -> Levels {
        let mut state = Levels::new();
        for cue in self.cues.iter().take(index.saturating_add(1)) {
            state = cue_state(cue, &state);
        }
        state
    }

    /// Accumulated state before cue `index`.
    pub fn state_before(&self, index: usize) -> Levels {
        let mut state = Levels::new();
        for cue in self.cues.iter().take(index) {
            state = cue_state(cue, &state);
        }
        state
    }

    /// Rewrite every cue's stored levels so the accumulated state at each cue is
    /// unchanged, respecting blocked cues and shields.
    ///
    /// Every structural edit (move, copy, delete, block/unblock) and every store
    /// that can change an earlier cue goes through this. Without it, reordering
    /// deltas silently changes looks, and a shield would only hold until the
    /// next edit upstream.
    pub fn rebase(&mut self) {
        let mut state = Levels::new();
        for cue in &mut self.cues {
            let absolute = cue_state(cue, &state);
            cue.levels = if cue.blocked {
                absolute.clone()
            } else {
                absolute.difference(&state)
            };
            state = absolute;
        }
    }

    /// Remove a shield and put the attribute back into tracking. The level a
    /// rebase baked into this cue's delta belonged to the shield, so it is
    /// dropped; the cue inherits the upstream value again.
    pub fn remove_shield(&mut self, index: usize, attribute: &str) {
        let Some(cue) = self.cues.get_mut(index) else {
            return;
        };
        let key = cue
            .shield
            .keys()
            .find(|name| name.eq_ignore_ascii_case(attribute))
            .cloned();
        if let Some(key) = key {
            cue.shield.remove(&key);
        }
        if !cue.blocked {
            let fixtures: Vec<u16> = cue.levels.fixtures().map(|(number, _)| number).collect();
            for fixture in fixtures {
                cue.levels.remove(fixture, attribute);
            }
        }
        self.rebase();
    }
}

/// The complete state a cue leaves behind.
///
/// A blocked cue replaces the inherited state; a tracking cue overlays its
/// delta. Shields are applied last so the stored level wins over whatever the
/// accumulated state says.
fn cue_state(cue: &Cue, inherited: &Levels) -> Levels {
    let mut absolute = if cue.blocked {
        cue.levels.clone()
    } else {
        let mut absolute = inherited.clone();
        absolute.overlay(&cue.levels);
        absolute
    };
    for (attribute, level) in &cue.shield {
        let fixtures: Vec<u16> = absolute
            .fixtures()
            .filter(|(_, attributes)| {
                attributes
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case(attribute))
            })
            .map(|(number, _)| number)
            .collect();
        for fixture in fixtures {
            absolute.set(fixture, attribute, *level);
        }
    }
    absolute
}

fn page_one() -> u16 {
    1
}

fn is_page_one(page: &u16) -> bool {
    *page == 1
}

/// A playback can be started from a sequence directly or through an executor.
///
/// An executor target names its page as well as its number: the same number on
/// two pages is two different playbacks, and a page switch must not steal one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Sequence(u16),
    Executor { page: u16, number: u16 },
}

impl Target {
    pub fn executor(page: u16, number: u16) -> Self {
        Target::Executor { page, number }
    }

    pub fn label(self) -> String {
        match self {
            Target::Sequence(number) => format!("Sequence {number}"),
            Target::Executor { page: 1, number } => format!("Executor {number}"),
            Target::Executor { page, number } => format!("Page {page} Executor {number}"),
        }
    }
}

/// One attribute's move between two values.
///
/// A whole cue becomes a list of these; a cue with no per-attribute timing and
/// no stagger keeps the single blend instead, so shows written before this
/// existed render through the same arithmetic they always did.
#[derive(Debug, Clone)]
struct Lane {
    fixture: u16,
    attribute: String,
    from: f64,
    to: f64,
    /// Cue delay plus the attribute's own delay plus its stagger offset.
    start: Duration,
    fade: Duration,
    curve: Curve,
}

impl Lane {
    fn value_at(&self, elapsed: Duration) -> f64 {
        if elapsed <= self.start {
            return self.from;
        }
        if self.fade.is_zero() || self.curve == Curve::Snap {
            return self.to;
        }
        let faded = (elapsed - self.start).as_secs_f64() / self.fade.as_secs_f64();
        let t = faded.clamp(0.0, 1.0);
        (self.from + (self.to - self.from) * self.curve.apply(t)).clamp(0.0, 1.0)
    }

    fn progress_at(&self, elapsed: Duration) -> f64 {
        if elapsed <= self.start {
            return 0.0;
        }
        if self.fade.is_zero() || self.curve == Curve::Snap {
            return 1.0;
        }
        ((elapsed - self.start).as_secs_f64() / self.fade.as_secs_f64()).clamp(0.0, 1.0)
    }
}

/// A run held by a hand: the frozen look plus exactly what `resume` needs.
///
/// One struct rather than four parallel fields, so a paused run cannot be half
/// set. `lanes` is `None` on the single-blend path, where `delay` and `fade`
/// carry the remaining times; with a lane plan they carry the rebased plan.
#[derive(Debug, Clone)]
struct Paused {
    levels: Levels,
    progress: f64,
    delay: Duration,
    fade: Duration,
    lanes: Option<Vec<Lane>>,
}

/// One running cue transition.
#[derive(Debug, Clone)]
pub struct Run {
    /// Cue number, not its position: deleting an earlier cue must not make the
    /// running playback point at the wrong cue.
    pub cue_number: f64,
    from: Levels,
    to: Levels,
    /// The per-attribute fade plan. Empty when every lane would move with the
    /// cue's own fade and delay at `Linear`, in which case the single blend is
    /// evaluated instead.
    lanes: Vec<Lane>,
    start: Instant,
    delay: Duration,
    fade: Duration,
    paused: Option<Paused>,
    pub done: bool,
    /// When the run began: `Time` triggers count from here.
    pub started: Instant,
    /// When the fade first reached 1.0: `Follow` triggers count from here. A
    /// paused or hand-driven run never settles, so it never triggers.
    pub settled: Option<Instant>,
}

impl Run {
    /// 0 before the delay ends, 1 once every lane has finished.
    pub fn progress(&self, now: Instant) -> f64 {
        if let Some(paused) = &self.paused {
            return paused.progress;
        }
        let elapsed = now.saturating_duration_since(self.start);
        if self.lanes.is_empty() {
            if elapsed < self.delay {
                return 0.0;
            }
            let faded = elapsed - self.delay;
            if self.fade.is_zero() {
                return 1.0;
            }
            return (faded.as_secs_f64() / self.fade.as_secs_f64()).clamp(0.0, 1.0);
        }
        // The slowest lane decides when the cue is settled, so a quick colour
        // fade cannot make a `Follow` fire while a long move is still running.
        self.lanes
            .iter()
            .map(|lane| lane.progress_at(elapsed))
            .fold(1.0, f64::min)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.is_some()
    }

    /// Output of this playback at `now`.
    pub fn levels_at(&self, now: Instant) -> Levels {
        if let Some(paused) = &self.paused {
            return paused.levels.clone();
        }
        if self.lanes.is_empty() {
            return self.levels_at_progress(self.progress(now));
        }
        let elapsed = now.saturating_duration_since(self.start);
        let mut levels = self.to.clone();
        for lane in &self.lanes {
            levels.set(lane.fixture, &lane.attribute, lane.value_at(elapsed));
        }
        levels
    }

    /// The look at a fraction of the fade, shared by time and by a hand on a
    /// fader so both produce the same blend. With a lane plan the fraction is
    /// each attribute's own progress; the cue's delays and stagger do not
    /// stretch the hand.
    fn levels_at_progress(&self, progress: f64) -> Levels {
        if progress <= 0.0 {
            return self.from.clone();
        }
        if progress >= 1.0 {
            return self.to.clone();
        }
        Levels::blend(&self.from, &self.to, progress)
    }

    fn levels_at_fraction(&self, fraction: f64) -> Levels {
        if self.lanes.is_empty() {
            return self.levels_at_progress(fraction);
        }
        let fraction = clamp_fraction(fraction);
        let mut levels = self.to.clone();
        for lane in &self.lanes {
            let value = if fraction <= 0.0 {
                lane.from
            } else if fraction >= 1.0 {
                lane.to
            } else {
                lane.from + (lane.to - lane.from) * lane.curve.apply(fraction)
            };
            levels.set(lane.fixture, &lane.attribute, value.clamp(0.0, 1.0));
        }
        levels
    }

    /// Lanes that continue a held run from the frozen look: each keeps its
    /// remaining delay and the fade time left at `elapsed`.
    fn held_lanes(&self, elapsed: Duration) -> Vec<Lane> {
        self.lanes
            .iter()
            .map(|lane| {
                let value = lane.value_at(elapsed);
                let progress = lane.progress_at(elapsed);
                let start = lane.start.saturating_sub(elapsed);
                let fade = if elapsed <= lane.start || lane.fade.is_zero() {
                    lane.fade
                } else {
                    lane.fade.mul_f64((1.0 - progress).clamp(0.0, 1.0))
                };
                Lane {
                    from: value,
                    start,
                    fade,
                    ..lane.clone()
                }
            })
            .collect()
    }

    /// Lanes that continue a hand-driven run from `fraction` of every fade.
    fn rebased_lanes(&self, fraction: f64) -> Vec<Lane> {
        let fraction = clamp_fraction(fraction);
        self.lanes
            .iter()
            .map(|lane| {
                let covered = lane.curve.apply(fraction);
                Lane {
                    from: lane.from + (lane.to - lane.from) * covered,
                    start: Duration::ZERO,
                    fade: lane.fade.mul_f64((1.0 - fraction).clamp(0.0, 1.0)),
                    ..lane.clone()
                }
            })
            .collect()
    }
}

/// A scrub fraction is a position in the fade: finite and within 0..=1.
fn clamp_fraction(fraction: f64) -> f64 {
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// All running playbacks, and the level output they produce together.
#[derive(Debug, Default)]
pub struct Playback {
    runs: BTreeMap<Target, Run>,
    output: Levels,
}

impl Playback {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run(&self, target: Target) -> Option<&Run> {
        self.runs.get(&target)
    }

    pub fn runs(&self) -> impl Iterator<Item = (Target, &Run)> {
        self.runs.iter().map(|(target, run)| (*target, run))
    }

    pub fn is_running(&self) -> bool {
        !self.runs.is_empty()
    }

    /// Combined output of every playback, scaled by executor faders.
    pub fn output(&self) -> &Levels {
        &self.output
    }

    /// Start (or restart) a cue on a target.
    pub fn go(
        &mut self,
        show: &Show,
        target: Target,
        sequence_number: u16,
        cue_index: usize,
        now: Instant,
    ) -> Result<(), String> {
        let sequence = show
            .sequence(sequence_number)
            .ok_or_else(|| format!("sequence {sequence_number} does not exist"))?;
        let cue = sequence
            .cues
            .get(cue_index)
            .ok_or_else(|| format!("sequence {sequence_number} has no cue at that position"))?;

        let mut from = match self.runs.get(&target) {
            Some(run) => run.levels_at(now),
            None => Levels::new(),
        };
        let to = sequence.state_at(cue_index);
        if cue.mib {
            snap_non_intensity(&mut from, &to);
        }
        let lanes = build_lanes(cue, &from, &to);
        self.runs.insert(
            target,
            Run {
                cue_number: cue.number,
                from,
                to,
                lanes,
                start: now,
                delay: seconds(cue.delay),
                fade: seconds(cue.fade),
                paused: None,
                done: false,
                started: now,
                settled: None,
            },
        );
        Ok(())
    }

    /// Move a run to `fraction` of its fade, as a hand on a fader does.
    ///
    /// The run is paused at the blended look, so pause/resume, release and
    /// output merging need no separate code path. A held run does not settle,
    /// so it never chains to the next cue by itself.
    pub fn scrub(&mut self, target: Target, fraction: f64) -> bool {
        let Some(run) = self.runs.get_mut(&target) else {
            return false;
        };
        let fraction = fraction.clamp(0.0, 1.0);
        run.paused = Some(Paused {
            levels: run.levels_at_fraction(fraction),
            progress: fraction,
            delay: Duration::ZERO,
            fade: if run.lanes.is_empty() {
                run.fade.mul_f64(1.0 - fraction)
            } else {
                Duration::ZERO
            },
            lanes: if run.lanes.is_empty() {
                None
            } else {
                Some(run.rebased_lanes(fraction))
            },
        });
        run.settled = None;
        true
    }

    /// Cues that should start now because the cue before them asked for it.
    ///
    /// The instant is explicit so tests never sleep. The index is the position
    /// of the cue to start; the caller starts it with [`Playback::go`], which
    /// replaces this target's run.
    pub fn due(&self, show: &Show, now: Instant) -> Vec<(Target, u16, usize)> {
        let mut due = Vec::new();
        for (target, run) in &self.runs {
            if run.is_paused() {
                continue;
            }
            let Some(sequence_number) = sequence_for(show, *target) else {
                continue;
            };
            let Some(sequence) = show.sequence(sequence_number) else {
                continue;
            };
            let Some(index) = sequence.cue_index(run.cue_number) else {
                continue;
            };
            // A chain never wraps: the last cue has no follower.
            let Some(next) = sequence.cues.get(index + 1) else {
                continue;
            };
            let when = match next.trigger {
                Trigger::Go => continue,
                // A follow counts from the settled fade, so a run that is still
                // moving has nothing to follow yet.
                Trigger::Follow => match run.settled {
                    Some(settled) => settled.checked_add(seconds(next.trigger_time)),
                    None => continue,
                },
                Trigger::Time => run.started.checked_add(seconds(next.trigger_time)),
            };
            if when.is_some_and(|when| now >= when) {
                due.push((*target, sequence_number, index + 1));
            }
        }
        due
    }

    /// Freeze a running cue where it is, remembering the delay and fade time
    /// that are still to come.
    pub fn pause(&mut self, target: Target, now: Instant) -> bool {
        match self.runs.get_mut(&target) {
            Some(run) if run.paused.is_none() => {
                let progress = run.progress(now);
                let levels = run.levels_at(now);
                let elapsed = now.saturating_duration_since(run.start);
                let delay = run.delay.saturating_sub(elapsed);
                let fade = run.fade.mul_f64((1.0 - progress).clamp(0.0, 1.0));
                run.paused = Some(Paused {
                    levels,
                    progress,
                    delay,
                    fade,
                    lanes: if run.lanes.is_empty() {
                        None
                    } else {
                        Some(run.held_lanes(elapsed))
                    },
                });
                // A held run is not settled, so it does not chain on its own.
                run.settled = None;
                true
            }
            _ => false,
        }
    }

    /// Continue a paused cue with the time that was left on its delay and fade.
    pub fn resume(&mut self, target: Target, now: Instant) -> bool {
        let Some(run) = self.runs.get_mut(&target) else {
            return false;
        };
        let Some(paused) = run.paused.take() else {
            return false;
        };
        run.start = now;
        match paused.lanes {
            Some(lanes) => {
                run.lanes = lanes;
                run.delay = Duration::ZERO;
                run.fade = Duration::ZERO;
            }
            None => {
                run.from = paused.levels;
                run.delay = paused.delay;
                run.fade = paused.fade;
            }
        }
        true
    }

    /// Stop a playback; its contribution disappears immediately.
    pub fn release(&mut self, target: Target) -> bool {
        self.runs.remove(&target).is_some()
    }

    /// Drop playbacks whose sequence or executor no longer exists, e.g. after
    /// an undo removed it.
    pub fn prune(&mut self, show: &Show) {
        self.runs.retain(|target, _| match target {
            Target::Sequence(number) => show.sequence(*number).is_some(),
            Target::Executor { page, number } => show.executor(*page, *number).is_some(),
        });
    }

    pub fn clear(&mut self) {
        self.runs.clear();
        self.output = Levels::new();
    }

    /// Which cue every target is on: the projection a desk file records.
    pub fn states(&self) -> Vec<(Target, f64, bool)> {
        self.runs
            .iter()
            .map(|(target, run)| (*target, run.cue_number, run.is_paused()))
            .collect()
    }

    /// Recompute the output for `now`. Returns true when it changed.
    ///
    /// A run that reaches the end of its fade records when it settled, which is
    /// what a `Follow` on the next cue counts from.
    pub fn refresh(&mut self, desk: &Desk, now: Instant) -> bool {
        for run in self.runs.values_mut() {
            if run.paused.is_some() {
                run.done = false;
                continue;
            }
            if run.progress(now) >= 1.0 {
                run.done = true;
                run.settled.get_or_insert(now);
            } else {
                run.done = false;
            }
        }

        let mut output = Levels::new();
        for (target, run) in &self.runs {
            let fader = match target {
                Target::Sequence(_) => desk.sequence_master(),
                Target::Executor { page, number } => desk.executor_master(*page, *number),
            };
            output.merge_htp(&run.levels_at(now).scale_intensity(fader));
        }

        let changed = output != self.output;
        self.output = output;
        changed
    }
}

/// Build the per-attribute fade plan for a cue.
///
/// Timing is looked up per attribute, the cue's fade and delay fill the gaps,
/// and the stagger index is the fixture's position in the cue's target state
/// rather than its number. Only values that actually change become lanes; the
/// rest come from the target state and cost nothing per frame. An empty plan
/// means every lane would have used the cue's own fade and delay at `Linear`,
/// which is what keeps a pre-v7 show on the single-blend path.
fn build_lanes(cue: &Cue, from: &Levels, to: &Levels) -> Vec<Lane> {
    // Fixtures and attributes of both states: an attribute that only exists in
    // `from` still has to fade to zero.
    let mut plan: Vec<(u16, Vec<String>)> = Vec::new();
    for (fixture, attributes) in to.fixtures() {
        plan.push((fixture, attributes.keys().cloned().collect()));
    }
    for (fixture, attributes) in from.fixtures() {
        match plan.iter_mut().find(|(number, _)| *number == fixture) {
            Some((_, names)) => {
                for name in attributes.keys() {
                    if !names
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(name))
                    {
                        names.push(name.clone());
                    }
                }
            }
            None => plan.push((fixture, attributes.keys().cloned().collect())),
        }
    }
    plan.sort_by_key(|(fixture, _)| *fixture);

    let cue_delay = seconds(cue.delay);
    let cue_fade = seconds(cue.fade);
    let mut lanes = Vec::new();
    for (position, (fixture, names)) in plan.into_iter().enumerate() {
        let offset = seconds(cue.stagger * position as f64);
        for name in names {
            let start = from.get(fixture, &name).unwrap_or(0.0);
            let end = to.get(fixture, &name).unwrap_or(0.0);
            if (start - end).abs() < 1e-12 {
                continue;
            }
            let timing = timing_for(cue, &name);
            lanes.push(Lane {
                fixture,
                attribute: name,
                from: start,
                to: end,
                // The cue delay is added once, and an attribute's own delay
                // adds to it; a missing override contributes nothing.
                start: cue_delay
                    + seconds(timing.and_then(|timing| timing.delay).unwrap_or(0.0))
                    + offset,
                fade: seconds(timing.and_then(|timing| timing.fade).unwrap_or(cue.fade)),
                curve: timing.map(|timing| timing.curve).unwrap_or_default(),
            });
        }
    }

    let plain = lanes.iter().all(|lane| {
        lane.start == cue_delay && lane.fade == cue_fade && lane.curve == Curve::Linear
    });
    if plain {
        return Vec::new();
    }
    lanes
}

/// Move-in-black: for every fixture that is dark in `from`, replace the
/// non-intensity attributes with the target values before the fade is built, so
/// they are constant for the whole transition. Intensity is left alone and
/// still fades; a fixture that is already lit is untouched.
fn snap_non_intensity(from: &mut Levels, to: &Levels) {
    for (fixture, attributes) in to.fixtures() {
        if !from.is_dark(fixture) {
            continue;
        }
        for (attribute, level) in attributes {
            if !is_intensity(attribute) {
                from.set(fixture, attribute, *level);
            }
        }
    }
}

/// The cue list a target plays.
fn sequence_for(show: &Show, target: Target) -> Option<u16> {
    match target {
        Target::Sequence(number) => Some(number),
        Target::Executor { page, number } => show
            .executor(page, number)
            .map(|executor| executor.sequence),
    }
}

fn seconds(value: f64) -> Duration {
    if value.is_finite() && value > 0.0 {
        Duration::from_secs_f64(value.min(MAX_TIME))
    } else {
        Duration::ZERO
    }
}

/// Cue numbers print without a trailing `.0`, and with the decimals they need.
pub fn format_cue_number(number: f64) -> String {
    if !number.is_finite() {
        return "?".to_string();
    }
    if (number - number.round()).abs() < 1e-9 {
        return format!("{}", number.round() as i64);
    }
    let text = format!("{number:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::demo_show;

    fn levels(values: &[(u16, &str, f64)]) -> Levels {
        let mut levels = Levels::new();
        for (fixture, attribute, level) in values {
            levels.set(*fixture, attribute, *level);
        }
        levels
    }

    fn cue(number: f64, fade: f64, levels: Levels) -> Cue {
        Cue {
            number,
            name: String::new(),
            fade,
            delay: 0.0,
            mib: false,
            trigger: Trigger::Go,
            trigger_time: 0.0,
            timing: BTreeMap::new(),
            stagger: 0.0,
            blocked: false,
            shield: BTreeMap::new(),
            levels,
        }
    }

    fn two_fixture_show() -> Show {
        let mut show = demo_show();
        let mut second = show.fixtures[0].clone();
        second.number = 2;
        second.address = 5;
        show.fixtures.push(second);
        show
    }

    fn sequence() -> Sequence {
        Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.5)])),
                cue(2.0, 1.0, levels(&[(1, "Dimmer", 1.0)])),
                cue(3.0, 1.0, levels(&[(2, "Dimmer", 1.0)])),
            ],
        }
    }

    #[test]
    fn cue_numbers_are_sorted_and_searchable() {
        let sequence = sequence();
        assert_eq!(sequence.cue_index(2.0), Some(1));
        assert_eq!(sequence.cue_index(2.5), None);
        assert_eq!(sequence.insertion_index(2.5), 2);
        assert_eq!(sequence.insertion_index(9.0), 3);
    }

    #[test]
    fn tracking_accumulates_earlier_cues() {
        let sequence = sequence();
        let first = sequence.state_at(0);
        assert_eq!(first.get(1, "Dimmer"), Some(0.5));
        let second = sequence.state_at(1);
        assert_eq!(second.get(1, "Dimmer"), Some(1.0));
        let third = sequence.state_at(2);
        assert_eq!(third.get(1, "Dimmer"), Some(1.0), "still tracked");
        assert_eq!(third.get(2, "Dimmer"), Some(1.0));
        assert_eq!(sequence.state_before(0).get(1, "Dimmer"), None);
        assert_eq!(sequence.state_before(2).get(2, "Dimmer"), None);
    }

    #[test]
    fn fade_interpolates_between_tracking_states() {
        let mut show = demo_show();
        show.sequences.push(sequence());
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();

        let at_start = playback.run(Target::Sequence(1)).unwrap().levels_at(start);
        assert_eq!(
            at_start.get(1, "Dimmer"),
            Some(0.5),
            "a zero fade is instant"
        );

        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let middle = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(500));
        assert_eq!(middle.get(1, "Dimmer"), Some(0.75));

        let end = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_secs(1));
        assert_eq!(end.get(1, "Dimmer"), Some(1.0));
    }

    #[test]
    fn delay_holds_the_previous_state() {
        let mut show = demo_show();
        let mut sequence = sequence();
        sequence.cues[1].delay = 1.0;
        show.sequences.push(sequence);
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        let after_first = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_secs(1));
        assert_eq!(after_first.get(1, "Dimmer"), Some(0.5));

        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let held = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(900));
        assert_eq!(held.get(1, "Dimmer"), Some(0.5), "delay keeps the old look");
        let moving = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(1500));
        assert_eq!(moving.get(1, "Dimmer"), Some(0.75));
    }

    #[test]
    fn move_in_black_snaps_colour_while_the_fixture_is_dark() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![
                cue(
                    1.0,
                    0.0,
                    levels(&[(1, "Dimmer", 0.0), (1, "ColorAdd_R", 0.0)]),
                ),
                Cue {
                    mib: true,
                    ..cue(
                        2.0,
                        1.0,
                        levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 1.0)]),
                    )
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let run = playback.run(Target::Sequence(1)).unwrap();
        let half = run.levels_at(start + Duration::from_millis(500));
        assert_eq!(half.get(1, "ColorAdd_R"), Some(1.0), "colour is in place");
        assert_eq!(half.get(1, "Dimmer"), Some(0.5), "intensity still fades");
    }

    #[test]
    fn move_in_black_leaves_a_lit_fixture_fading() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 1.0)])),
                Cue {
                    mib: true,
                    ..cue(
                        2.0,
                        1.0,
                        levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 1.0)]),
                    )
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let run = playback.run(Target::Sequence(1)).unwrap();
        let half = run.levels_at(start + Duration::from_millis(500));
        assert_eq!(half.get(1, "ColorAdd_R"), Some(0.5));
    }

    #[test]
    fn pause_freezes_and_resume_finishes_the_fade() {
        let mut show = demo_show();
        show.sequences.push(sequence());
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();

        assert!(playback.pause(Target::Sequence(1), start + Duration::from_millis(500)));
        let frozen = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_secs(5));
        assert_eq!(frozen.get(1, "Dimmer"), Some(0.75));

        assert!(playback.resume(Target::Sequence(1), start + Duration::from_millis(600)));
        let finished = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(1200));
        assert_eq!(finished.get(1, "Dimmer"), Some(1.0));
    }

    #[test]
    fn pause_during_a_delay_keeps_the_remaining_delay() {
        let mut show = demo_show();
        let mut sequence = sequence();
        sequence.cues[1].delay = 1.0;
        show.sequences.push(sequence);
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();

        assert!(playback.pause(Target::Sequence(1), start + Duration::from_millis(500)));
        assert!(playback.resume(Target::Sequence(1), start + Duration::from_millis(600)));

        let held = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(900));
        assert_eq!(
            held.get(1, "Dimmer"),
            Some(0.5),
            "500 ms of delay were left after resuming at 600 ms"
        );
        let moving = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(1600));
        assert_eq!(
            moving.get(1, "Dimmer"),
            Some(0.75),
            "half of the remaining fade"
        );
    }

    #[test]
    fn executor_fader_scales_intensity_only() {
        let mut show = demo_show();
        show.executors.push(Executor {
            page: 1,
            number: 1,
            sequence: 1,
            name: "Main".into(),
            mode: ExecutorMode::Go,
        });
        let sequence = Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![cue(
                1.0,
                0.0,
                levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 1.0)]),
            )],
        };
        show.sequences.push(sequence);

        let now = Instant::now();
        let mut desk = Desk::new();
        desk.set_executor_fader(1, 1, 0.5);
        let mut playback = Playback::new();
        playback
            .go(&show, Target::executor(1, 1), 1, 0, now)
            .unwrap();
        playback.refresh(&desk, now);
        assert_eq!(playback.output().get(1, "Dimmer"), Some(0.5));
        assert_eq!(playback.output().get(1, "ColorAdd_R"), Some(1.0));

        desk.set_executor_fader(1, 1, 1.0);
        assert!(playback.refresh(&desk, now), "fader change alters output");
        assert_eq!(playback.output().get(1, "Dimmer"), Some(1.0));
    }

    #[test]
    fn execute_go_from_a_sequence_needs_no_executor() {
        let mut show = demo_show();
        show.sequences.push(sequence());
        let now = Instant::now();
        let mut playback = Playback::new();
        playback.go(&show, Target::Sequence(1), 1, 0, now).unwrap();
        playback.refresh(&Desk::new(), now);
        assert_eq!(playback.output().get(1, "Dimmer"), Some(0.5));
        assert!(playback.release(Target::Sequence(1)));
        assert!(playback.refresh(&Desk::new(), now));
        assert!(playback.output().is_empty());
    }

    fn chain() -> Sequence {
        Sequence {
            number: 1,
            name: "Chain".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.4)])),
                Cue {
                    trigger: Trigger::Follow,
                    trigger_time: 1.0,
                    ..cue(2.0, 2.0, levels(&[(1, "Dimmer", 0.8)]))
                },
                Cue {
                    trigger: Trigger::Follow,
                    trigger_time: 0.0,
                    ..cue(3.0, 0.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        }
    }

    #[test]
    fn a_follow_waits_for_the_fade_to_settle() {
        let mut show = demo_show();
        show.sequences.push(chain());
        let start = Instant::now();
        let desk = Desk::new();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback.refresh(&desk, start);
        assert!(
            playback.due(&show, start).is_empty(),
            "the follow time has not passed"
        );
        assert_eq!(
            playback.due(&show, start + Duration::from_secs(1)),
            vec![(Target::Sequence(1), 1, 1)]
        );
    }

    #[test]
    fn a_follow_counts_from_the_end_of_the_fade_not_the_start() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Slow".into(),
            cues: vec![
                cue(1.0, 2.0, levels(&[(1, "Dimmer", 0.4)])),
                Cue {
                    trigger: Trigger::Follow,
                    trigger_time: 0.5,
                    ..cue(2.0, 0.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        });
        let start = Instant::now();
        let desk = Desk::new();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback.refresh(&desk, start + Duration::from_secs(1));
        assert!(playback
            .due(&show, start + Duration::from_secs(1))
            .is_empty());

        // The fade ends at 2 s; the follow time starts there.
        playback.refresh(&desk, start + Duration::from_secs(2));
        assert!(
            playback
                .due(&show, start + Duration::from_secs(2))
                .is_empty(),
            "half a second of follow time is still to run"
        );
        assert_eq!(
            playback
                .due(&show, start + Duration::from_millis(2500))
                .len(),
            1
        );
    }

    #[test]
    fn a_follow_zero_chains_on_the_same_tick() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Fast".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.4)])),
                Cue {
                    trigger: Trigger::Follow,
                    ..cue(2.0, 0.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        });
        let start = Instant::now();
        let desk = Desk::new();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback.refresh(&desk, start);
        assert_eq!(playback.due(&show, start).len(), 1);
    }

    #[test]
    fn a_time_trigger_counts_from_the_cue_start_and_overlaps() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Overlap".into(),
            cues: vec![
                cue(1.0, 4.0, levels(&[(1, "Dimmer", 0.4)])),
                Cue {
                    trigger: Trigger::Time,
                    trigger_time: 2.0,
                    ..cue(2.0, 4.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        });
        let start = Instant::now();
        let desk = Desk::new();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback.refresh(&desk, start + Duration::from_secs(1));
        assert!(playback
            .due(&show, start + Duration::from_secs(1))
            .is_empty());
        assert_eq!(
            playback.due(&show, start + Duration::from_secs(2)).len(),
            1,
            "a time trigger fires while the fade is still moving"
        );
    }

    #[test]
    fn a_go_cue_waits_for_a_button() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Manual".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.4)])),
                cue(2.0, 0.0, levels(&[(1, "Dimmer", 1.0)])),
            ],
        });
        let start = Instant::now();
        let desk = Desk::new();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback.refresh(&desk, start);
        assert!(playback
            .due(&show, start + Duration::from_secs(3600))
            .is_empty());
    }

    #[test]
    fn the_last_cue_never_wraps() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Single".into(),
            cues: vec![Cue {
                trigger: Trigger::Follow,
                ..cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.4)]))
            }],
        });
        let start = Instant::now();
        let desk = Desk::new();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback.refresh(&desk, start);
        assert!(playback
            .due(&show, start + Duration::from_secs(3600))
            .is_empty());
    }

    #[test]
    fn a_held_run_never_triggers() {
        let mut show = demo_show();
        show.sequences.push(chain());
        let start = Instant::now();
        let desk = Desk::new();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        assert!(playback.pause(Target::Sequence(1), start));
        playback.refresh(&desk, start + Duration::from_secs(10));
        assert!(
            playback
                .due(&show, start + Duration::from_secs(20))
                .is_empty(),
            "a run held by a hand does not chain"
        );
    }

    #[test]
    fn scrub_follows_the_blend_at_any_fraction() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Hand".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.4)])),
                cue(2.0, 4.0, levels(&[(1, "Dimmer", 1.0)])),
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();

        assert!(playback.scrub(Target::Sequence(1), 0.5));
        let run = playback.run(Target::Sequence(1)).unwrap();
        assert!(run.is_paused());
        assert_eq!(
            run.levels_at(start + Duration::from_secs(99))
                .get(1, "Dimmer"),
            Some(0.7),
            "half the fade is the same blend time would produce"
        );

        playback.scrub(Target::Sequence(1), 0.0);
        assert_eq!(
            playback
                .run(Target::Sequence(1))
                .unwrap()
                .levels_at(start)
                .get(1, "Dimmer"),
            Some(0.4)
        );
        playback.scrub(Target::Sequence(1), 1.0);
        assert_eq!(
            playback
                .run(Target::Sequence(1))
                .unwrap()
                .levels_at(start)
                .get(1, "Dimmer"),
            Some(1.0)
        );
    }

    #[test]
    fn a_per_attribute_fade_finishes_before_the_cue() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Timing".into(),
            cues: vec![
                cue(
                    1.0,
                    0.0,
                    levels(&[(1, "Dimmer", 0.0), (1, "ColorAdd_R", 0.0)]),
                ),
                Cue {
                    timing: BTreeMap::from([(
                        "ColorAdd_R".to_string(),
                        AttributeTiming {
                            fade: Some(0.5),
                            ..AttributeTiming::default()
                        },
                    )]),
                    ..cue(
                        2.0,
                        2.0,
                        levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 1.0)]),
                    )
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let half = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(500));
        assert_eq!(half.get(1, "ColorAdd_R"), Some(1.0), "colour is in place");
        assert_eq!(
            half.get(1, "Dimmer"),
            Some(0.25),
            "the dimmer is still moving"
        );
    }

    #[test]
    fn a_per_attribute_delay_adds_to_the_cue_delay() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Timing".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.0)])),
                Cue {
                    delay: 1.0,
                    timing: BTreeMap::from([(
                        "Dimmer".to_string(),
                        AttributeTiming {
                            delay: Some(1.0),
                            ..AttributeTiming::default()
                        },
                    )]),
                    ..cue(2.0, 2.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let run = playback.run(Target::Sequence(1)).unwrap();
        assert!(
            run.progress(start + Duration::from_millis(1900)) < 1.0,
            "a lane that has not started keeps the cue unsettled"
        );
        assert_eq!(
            run.levels_at(start + Duration::from_millis(1900))
                .get(1, "Dimmer"),
            Some(0.0),
            "the cue delay alone was not enough"
        );
        assert_eq!(
            run.levels_at(start + Duration::from_secs(3))
                .get(1, "Dimmer"),
            Some(0.5),
            "half of the fade after both delays"
        );
    }

    #[test]
    fn a_stagger_offsets_fixtures_in_target_order() {
        let mut show = two_fixture_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Stagger".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.0), (2, "Dimmer", 0.0)])),
                Cue {
                    stagger: 0.5,
                    ..cue(2.0, 1.0, levels(&[(1, "Dimmer", 1.0), (2, "Dimmer", 1.0)]))
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let run = playback.run(Target::Sequence(1)).unwrap();
        let at_half = run.levels_at(start + Duration::from_millis(500));
        assert_eq!(at_half.get(1, "Dimmer"), Some(0.5));
        assert_eq!(at_half.get(2, "Dimmer"), Some(0.0), "fixture 2 waits");
        let later = run.levels_at(start + Duration::from_millis(1250));
        assert_eq!(later.get(1, "Dimmer"), Some(1.0));
        assert_eq!(later.get(2, "Dimmer"), Some(0.75));
    }

    #[test]
    fn a_snap_curve_changes_at_its_start() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Snap".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.0)])),
                Cue {
                    timing: BTreeMap::from([(
                        "Dimmer".to_string(),
                        AttributeTiming {
                            delay: Some(1.0),
                            curve: Curve::Snap,
                            ..AttributeTiming::default()
                        },
                    )]),
                    ..cue(2.0, 3.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let run = playback.run(Target::Sequence(1)).unwrap();
        assert_eq!(
            run.levels_at(start + Duration::from_millis(999))
                .get(1, "Dimmer"),
            Some(0.0)
        );
        assert_eq!(
            run.levels_at(start + Duration::from_millis(1001))
                .get(1, "Dimmer"),
            Some(1.0),
            "a snap ignores its fade time"
        );
    }

    #[test]
    fn ease_out_is_ahead_of_linear_at_the_midpoint() {
        assert_eq!(Curve::EaseOut.apply(0.0), 0.0);
        assert_eq!(Curve::EaseOut.apply(1.0), 1.0);
        assert_eq!(Curve::EaseOut.apply(0.5), 0.75);
        assert_eq!(Curve::EaseIn.apply(0.5), 0.25);
        assert_eq!(Curve::EaseInOut.apply(0.5), 0.5);
        assert_eq!(Curve::Snap.apply(0.0), 0.0);
        assert_eq!(Curve::Snap.apply(0.01), 1.0);

        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Ease".into(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.0)])),
                Cue {
                    timing: BTreeMap::from([(
                        "Dimmer".to_string(),
                        AttributeTiming {
                            curve: Curve::EaseOut,
                            ..AttributeTiming::default()
                        },
                    )]),
                    ..cue(2.0, 2.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let run = playback.run(Target::Sequence(1)).unwrap();
        assert_eq!(
            run.levels_at(start + Duration::from_secs(1))
                .get(1, "Dimmer"),
            Some(0.75)
        );
        assert_eq!(
            run.levels_at(start + Duration::from_secs(2))
                .get(1, "Dimmer"),
            Some(1.0)
        );
    }

    #[test]
    fn an_untimed_attribute_holds_its_target() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Base".into(),
            cues: vec![
                cue(
                    1.0,
                    0.0,
                    levels(&[(1, "Dimmer", 0.0), (1, "ColorAdd_R", 1.0)]),
                ),
                Cue {
                    timing: BTreeMap::from([(
                        "Dimmer".to_string(),
                        AttributeTiming {
                            fade: Some(1.0),
                            ..AttributeTiming::default()
                        },
                    )]),
                    ..cue(2.0, 1.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let half = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(500));
        assert_eq!(
            half.get(1, "ColorAdd_R"),
            Some(1.0),
            "a value the cue does not change is not part of the plan"
        );
        assert_eq!(half.get(1, "Dimmer"), Some(0.5));
    }

    #[test]
    fn a_plain_cue_blends_exactly_like_a_single_fade() {
        let mut show = demo_show();
        let from = levels(&[(1, "Dimmer", 0.0), (1, "ColorAdd_B", 1.0)]);
        let to = levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 0.25)]);
        let delta = to.difference(&from);
        show.sequences.push(Sequence {
            number: 1,
            name: "Plain".into(),
            cues: vec![cue(1.0, 0.0, from.clone()), cue(2.0, 4.0, delta)],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        let target = show.sequences[0].state_at(1);
        let quarter = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_secs(1));
        assert_eq!(quarter, Levels::blend(&from, &target, 0.25));
    }

    #[test]
    fn pausing_mid_ease_resumes_to_the_same_end() {
        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Hold".into(),
            cues: vec![
                cue(
                    1.0,
                    0.0,
                    levels(&[(1, "Dimmer", 0.0), (1, "ColorAdd_R", 0.0)]),
                ),
                Cue {
                    timing: BTreeMap::from([(
                        "ColorAdd_R".to_string(),
                        AttributeTiming {
                            fade: Some(0.5),
                            ..AttributeTiming::default()
                        },
                    )]),
                    ..cue(
                        2.0,
                        2.0,
                        levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 1.0)]),
                    )
                },
            ],
        });
        let start = Instant::now();
        let mut playback = Playback::new();
        playback
            .go(&show, Target::Sequence(1), 1, 0, start)
            .unwrap();
        playback
            .go(&show, Target::Sequence(1), 1, 1, start)
            .unwrap();
        assert!(playback.pause(Target::Sequence(1), start + Duration::from_millis(250)));
        let frozen = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_secs(9));
        assert_eq!(frozen.get(1, "ColorAdd_R"), Some(0.5));

        assert!(playback.resume(Target::Sequence(1), start + Duration::from_secs(1)));
        let resumed = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(1250));
        assert_eq!(resumed.get(1, "ColorAdd_R"), Some(1.0));
        let finished = playback
            .run(Target::Sequence(1))
            .unwrap()
            .levels_at(start + Duration::from_millis(2750));
        assert_eq!(finished.get(1, "Dimmer"), Some(1.0));
    }

    #[test]
    fn an_unknown_trigger_name_is_a_deserialization_error() {
        let error = serde_json::from_str::<Cue>(r#"{"number":1,"trigger":"sound"}"#).unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }

    #[test]
    fn cue_numbers_format_without_noise() {
        assert_eq!(format_cue_number(1.0), "1");
        assert_eq!(format_cue_number(1.5), "1.5");
        assert_eq!(format_cue_number(2.25), "2.25");
        assert_eq!(format_cue_number(2.125), "2.125");
        assert_eq!(format_cue_number(f64::NAN), "?");
    }

    fn states(sequence: &Sequence) -> Vec<Levels> {
        (0..sequence.cues.len())
            .map(|index| sequence.state_at(index))
            .collect()
    }

    #[test]
    fn rebase_preserves_every_cue_and_is_idempotent() {
        let mut sequence = Sequence {
            number: 1,
            name: String::new(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.4)])),
                cue(
                    2.0,
                    0.0,
                    levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 0.5)]),
                ),
                cue(3.0, 0.0, levels(&[(2, "Dimmer", 0.8)])),
                Cue {
                    blocked: true,
                    levels: levels(&[(1, "Dimmer", 0.2), (2, "ColorAdd_R", 1.0)]),
                    ..cue(4.0, 0.0, Levels::new())
                },
            ],
        };
        let before = states(&sequence);
        sequence.rebase();
        assert_eq!(states(&sequence), before);

        let stored: Vec<Levels> = sequence.cues.iter().map(|cue| cue.levels.clone()).collect();
        sequence.rebase();
        assert_eq!(
            sequence
                .cues
                .iter()
                .map(|cue| cue.levels.clone())
                .collect::<Vec<_>>(),
            stored,
            "a second rebase changes nothing"
        );
    }

    /// A small deterministic generator, so the property test needs no crate.
    fn pick(seed: &mut u64, bound: u64) -> u64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (*seed >> 33) % bound
    }

    #[test]
    fn rebase_preserves_state_for_generated_sequences() {
        let attributes = ["Dimmer", "ColorAdd_R", "ColorAdd_G"];
        let mut seed = 0x5eed_u64;
        for _ in 0..250 {
            let length = 1 + pick(&mut seed, 7) as usize;
            let mut cues = Vec::new();
            for index in 0..length {
                let mut levels = Levels::new();
                for _ in 0..pick(&mut seed, 4) {
                    let fixture = 1 + pick(&mut seed, 2) as u16;
                    let attribute = attributes[pick(&mut seed, attributes.len() as u64) as usize];
                    let level = pick(&mut seed, 101) as f64 / 100.0;
                    levels.set(fixture, attribute, level);
                }
                let mut shield = BTreeMap::new();
                if pick(&mut seed, 4) == 0 {
                    let attribute = attributes[pick(&mut seed, attributes.len() as u64) as usize];
                    shield.insert(attribute.to_string(), pick(&mut seed, 101) as f64 / 100.0);
                }
                cues.push(Cue {
                    blocked: pick(&mut seed, 6) == 0,
                    shield,
                    ..cue(index as f64 + 1.0, 0.0, levels)
                });
            }
            let mut sequence = Sequence {
                number: 1,
                name: String::new(),
                cues,
            };
            let before = states(&sequence);
            let before_indices: Vec<Levels> = (0..sequence.cues.len())
                .map(|index| sequence.state_before(index))
                .collect();
            sequence.rebase();
            assert_eq!(states(&sequence), before);
            assert_eq!(
                (0..sequence.cues.len())
                    .map(|index| sequence.state_before(index))
                    .collect::<Vec<_>>(),
                before_indices
            );
        }
    }

    #[test]
    fn a_blocked_cue_ignores_a_later_upstream_edit() {
        let mut sequence = Sequence {
            number: 1,
            name: String::new(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.4)])),
                Cue {
                    blocked: true,
                    levels: levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 0.2)]),
                    ..cue(2.0, 0.0, Levels::new())
                },
            ],
        };
        sequence.rebase();
        sequence.cues[0].levels = levels(&[(1, "Dimmer", 0.9), (1, "ColorAdd_R", 0.8)]);
        sequence.cues[0].blocked = false;
        sequence.rebase();
        let state = sequence.state_at(1);
        assert_eq!(state.get(1, "Dimmer"), Some(1.0));
        assert_eq!(state.get(1, "ColorAdd_R"), Some(0.2));
    }

    #[test]
    fn a_shield_holds_while_other_attributes_ripple() {
        let mut sequence = Sequence {
            number: 1,
            name: String::new(),
            cues: vec![
                cue(
                    1.0,
                    0.0,
                    levels(&[(1, "Dimmer", 0.4), (1, "ColorAdd_R", 0.5)]),
                ),
                Cue {
                    shield: BTreeMap::from([("ColorAdd_R".to_string(), 0.5)]),
                    ..cue(2.0, 0.0, levels(&[(1, "Dimmer", 1.0)]))
                },
            ],
        };
        sequence.rebase();
        sequence.cues[0].levels = levels(&[(1, "Dimmer", 0.9), (1, "ColorAdd_R", 1.0)]);
        sequence.rebase();
        let state = sequence.state_at(1);
        assert_eq!(state.get(1, "Dimmer"), Some(1.0));
        assert_eq!(state.get(1, "ColorAdd_R"), Some(0.5), "the shield holds");

        sequence.remove_shield(1, "ColorAdd_R");
        assert_eq!(
            sequence.state_at(1).get(1, "ColorAdd_R"),
            Some(1.0),
            "without the shield the upstream value comes through"
        );
    }

    #[test]
    fn unblock_restores_delta_behaviour() {
        let mut sequence = Sequence {
            number: 1,
            name: String::new(),
            cues: vec![
                cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.4)])),
                Cue {
                    blocked: true,
                    levels: levels(&[(1, "Dimmer", 1.0)]),
                    ..cue(2.0, 0.0, Levels::new())
                },
            ],
        };
        sequence.rebase();
        sequence.cues[0].levels = levels(&[(1, "Dimmer", 0.4), (1, "ColorAdd_R", 0.7)]);
        sequence.rebase();
        assert_eq!(
            sequence.state_at(1).get(1, "ColorAdd_R"),
            None,
            "a blocked cue does not inherit the later colour"
        );

        sequence.cues[1].blocked = false;
        sequence.rebase();
        assert_eq!(
            sequence.state_at(1).get(1, "ColorAdd_R"),
            Some(0.7),
            "unblocked, the cue tracks the upstream colour again"
        );
    }

    #[test]
    fn the_same_number_on_two_pages_is_two_playbacks() {
        let mut show = demo_show();
        show.pages.push(crate::show::Page::new(2));
        show.sequences.push(Sequence {
            number: 1,
            name: String::new(),
            cues: vec![cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.25)]))],
        });
        show.sequences.push(Sequence {
            number: 2,
            name: String::new(),
            cues: vec![cue(1.0, 0.0, levels(&[(1, "Dimmer", 0.75)]))],
        });
        show.executors.push(Executor {
            page: 1,
            number: 1,
            sequence: 1,
            name: String::new(),
            mode: ExecutorMode::Go,
        });
        show.executors.push(Executor {
            page: 2,
            number: 1,
            sequence: 2,
            name: String::new(),
            mode: ExecutorMode::Go,
        });

        let now = Instant::now();
        let mut desk = Desk::new();
        desk.set_executor_fader(1, 1, 0.5);
        desk.set_executor_fader(2, 1, 0.5);
        let mut playback = Playback::new();
        playback
            .go(&show, Target::executor(1, 1), 1, 0, now)
            .unwrap();
        playback
            .go(&show, Target::executor(2, 1), 2, 0, now)
            .unwrap();
        playback.refresh(&desk, now);

        assert_eq!(playback.runs().count(), 2, "one run per page");
        assert_eq!(
            playback.run(Target::executor(1, 1)).unwrap().cue_number,
            1.0
        );
        assert_eq!(
            playback.run(Target::executor(2, 1)).unwrap().cue_number,
            1.0
        );
        assert_eq!(
            playback.output().get(1, "Dimmer"),
            Some(0.375),
            "HTP of 12.5% and 37.5%"
        );
        assert_eq!(Target::executor(1, 1).label(), "Executor 1");
        assert_eq!(Target::executor(2, 1).label(), "Page 2 Executor 1");

        assert!(playback.release(Target::executor(2, 1)));
        playback.refresh(&desk, now);
        assert_eq!(playback.output().get(1, "Dimmer"), Some(0.125));
        assert!(
            playback.run(Target::executor(1, 1)).is_some(),
            "releasing page 2 leaves page 1 running"
        );
    }
}

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
            state.overlay(&cue.levels);
        }
        state
    }

    /// Accumulated state before cue `index`.
    pub fn state_before(&self, index: usize) -> Levels {
        let mut state = Levels::new();
        for cue in self.cues.iter().take(index) {
            state.overlay(&cue.levels);
        }
        state
    }
}

/// A playback can be started from a sequence directly or through an executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Sequence(u16),
    Executor(u16),
}

impl Target {
    pub fn label(self) -> String {
        match self {
            Target::Sequence(number) => format!("Sequence {number}"),
            Target::Executor(number) => format!("Executor {number}"),
        }
    }
}

/// One running cue transition.
#[derive(Debug, Clone)]
pub struct Run {
    /// Cue number, not its position: deleting an earlier cue must not make the
    /// running playback point at the wrong cue.
    pub cue_number: f64,
    from: Levels,
    to: Levels,
    start: Instant,
    delay: Duration,
    fade: Duration,
    paused: Option<Levels>,
    paused_progress: f64,
    paused_delay: Duration,
    paused_fade: Duration,
    pub done: bool,
    /// When the run began: `Time` triggers count from here.
    pub started: Instant,
    /// When the fade first reached 1.0: `Follow` triggers count from here. A
    /// paused or hand-driven run never settles, so it never triggers.
    pub settled: Option<Instant>,
}

impl Run {
    /// 0 before the delay ends, 1 once the fade has finished.
    pub fn progress(&self, now: Instant) -> f64 {
        if self.paused.is_some() {
            return self.paused_progress;
        }
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed < self.delay {
            return 0.0;
        }
        let faded = elapsed - self.delay;
        if self.fade.is_zero() {
            return 1.0;
        }
        (faded.as_secs_f64() / self.fade.as_secs_f64()).clamp(0.0, 1.0)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.is_some()
    }

    /// Output of this playback at `now`.
    pub fn levels_at(&self, now: Instant) -> Levels {
        if let Some(levels) = &self.paused {
            return levels.clone();
        }
        self.levels_at_progress(self.progress(now))
    }

    /// The look at a fraction of the fade, shared by time and by a hand on a
    /// fader so both produce the same blend.
    fn levels_at_progress(&self, progress: f64) -> Levels {
        if progress <= 0.0 {
            return self.from.clone();
        }
        if progress >= 1.0 {
            return self.to.clone();
        }
        Levels::blend(&self.from, &self.to, progress)
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
        self.runs.insert(
            target,
            Run {
                cue_number: cue.number,
                from,
                to,
                start: now,
                delay: seconds(cue.delay),
                fade: seconds(cue.fade),
                paused: None,
                paused_progress: 1.0,
                paused_delay: Duration::ZERO,
                paused_fade: Duration::ZERO,
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
        run.paused = Some(run.levels_at_progress(fraction));
        run.paused_progress = fraction;
        run.paused_delay = Duration::ZERO;
        run.paused_fade = run.fade.mul_f64(1.0 - fraction);
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
                run.paused_delay = run.delay.saturating_sub(elapsed);
                run.paused_fade = run.fade.mul_f64((1.0 - progress).clamp(0.0, 1.0));
                run.paused = Some(levels);
                run.paused_progress = progress;
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
        let Some(levels) = run.paused.take() else {
            return false;
        };
        run.from = levels;
        run.delay = run.paused_delay;
        run.fade = run.paused_fade;
        run.start = now;
        run.paused_progress = 1.0;
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
            Target::Executor(number) => show.executor(*number).is_some(),
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
                Target::Executor(number) => desk.executor_master(*number),
            };
            output.merge_htp(&run.levels_at(now).scale_intensity(fader));
        }

        let changed = output != self.output;
        self.output = output;
        changed
    }
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
        Target::Executor(number) => show.executor(number).map(|executor| executor.sequence),
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
            levels,
        }
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
        desk.set_executor_fader(1, 0.5);
        let mut playback = Playback::new();
        playback.go(&show, Target::Executor(1), 1, 0, now).unwrap();
        playback.refresh(&desk, now);
        assert_eq!(playback.output().get(1, "Dimmer"), Some(0.5));
        assert_eq!(playback.output().get(1, "ColorAdd_R"), Some(1.0));

        desk.set_executor_fader(1, 1.0);
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
}

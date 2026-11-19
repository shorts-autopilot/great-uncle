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

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::levels::Levels;
use crate::show::Show;

/// Longest fade or delay accepted, in seconds (24 hours).
pub const MAX_TIME: f64 = 86_400.0;

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
    /// Tracked values: only what changes relative to the previous cue.
    #[serde(default)]
    pub levels: Levels,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Executor {
    pub number: u16,
    pub sequence: u16,
    #[serde(default)]
    pub name: String,
    /// Master fader, 0..=1. Scales intensity only.
    #[serde(default = "full_fader")]
    pub fader: f64,
}

fn full_fader() -> f64 {
    1.0
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

    pub fn number(self) -> u16 {
        match self {
            Target::Sequence(number) | Target::Executor(number) => number,
        }
    }
}

/// One running cue transition.
#[derive(Debug, Clone)]
pub struct Run {
    pub target: Target,
    pub sequence: u16,
    pub cue_index: usize,
    from: Levels,
    to: Levels,
    start: Instant,
    delay: Duration,
    fade: Duration,
    paused: Option<Levels>,
    paused_progress: f64,
    pub done: bool,
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

    pub fn fade(&self) -> Duration {
        self.fade
    }

    pub fn delay(&self) -> Duration {
        self.delay
    }

    /// Output of this playback at `now`.
    pub fn levels_at(&self, now: Instant) -> Levels {
        if let Some(levels) = &self.paused {
            return levels.clone();
        }
        let progress = self.progress(now);
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

        let from = match self.runs.get(&target) {
            Some(run) => run.levels_at(now),
            None => Levels::new(),
        };
        let to = sequence.state_at(cue_index);
        self.runs.insert(
            target,
            Run {
                target,
                sequence: sequence_number,
                cue_index,
                from,
                to,
                start: now,
                delay: seconds(cue.delay),
                fade: seconds(cue.fade),
                paused: None,
                paused_progress: 1.0,
                done: false,
            },
        );
        Ok(())
    }

    /// Freeze a running cue where it is.
    pub fn pause(&mut self, target: Target, now: Instant) -> bool {
        match self.runs.get_mut(&target) {
            Some(run) if run.paused.is_none() => {
                let progress = run.progress(now);
                let levels = run.levels_at(now);
                run.paused = Some(levels);
                run.paused_progress = progress;
                true
            }
            _ => false,
        }
    }

    /// Continue a paused cue for the time that was left on its fade.
    pub fn resume(&mut self, target: Target, now: Instant) -> bool {
        let Some(run) = self.runs.get_mut(&target) else {
            return false;
        };
        let Some(levels) = run.paused.take() else {
            return false;
        };
        let remaining = run
            .fade
            .mul_f64((1.0 - run.paused_progress).clamp(0.0, 1.0));
        run.from = levels;
        run.delay = Duration::ZERO;
        run.fade = remaining;
        run.start = now;
        run.paused_progress = 1.0;
        true
    }

    /// Stop a playback; its contribution disappears immediately.
    pub fn release(&mut self, target: Target) -> bool {
        self.runs.remove(&target).is_some()
    }

    pub fn clear(&mut self) {
        self.runs.clear();
        self.output = Levels::new();
    }

    /// Recompute the output for `now`. Returns true when it changed.
    pub fn refresh(&mut self, show: &Show, now: Instant) -> bool {
        for run in self.runs.values_mut() {
            run.done = run.paused.is_none() && run.progress(now) >= 1.0;
        }

        let mut output = Levels::new();
        for (target, run) in &self.runs {
            let fader = match target {
                Target::Sequence(_) => 1.0,
                Target::Executor(number) => show
                    .executor(*number)
                    .map(|executor| executor.fader)
                    .unwrap_or(1.0),
            };
            output.merge_htp(&run.levels_at(now).scale_intensity(fader));
        }

        let changed = output != self.output;
        self.output = output;
        changed
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
    fn executor_fader_scales_intensity_only() {
        let mut show = demo_show();
        show.executors.push(Executor {
            number: 1,
            sequence: 1,
            name: "Main".into(),
            fader: 0.5,
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
        let mut playback = Playback::new();
        playback.go(&show, Target::Executor(1), 1, 0, now).unwrap();
        playback.refresh(&show, now);
        assert_eq!(playback.output().get(1, "Dimmer"), Some(0.5));
        assert_eq!(playback.output().get(1, "ColorAdd_R"), Some(1.0));

        show.executors[0].fader = 1.0;
        assert!(playback.refresh(&show, now), "fader change alters output");
        assert_eq!(playback.output().get(1, "Dimmer"), Some(1.0));
    }

    #[test]
    fn execute_go_from_a_sequence_needs_no_executor() {
        let mut show = demo_show();
        show.sequences.push(sequence());
        let now = Instant::now();
        let mut playback = Playback::new();
        playback.go(&show, Target::Sequence(1), 1, 0, now).unwrap();
        playback.refresh(&show, now);
        assert_eq!(playback.output().get(1, "Dimmer"), Some(0.5));
        assert!(playback.release(Target::Sequence(1)));
        assert!(playback.refresh(&show, now));
        assert!(playback.output().is_empty());
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

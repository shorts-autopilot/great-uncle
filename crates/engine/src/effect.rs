//! Parameter effects (phasers).
//!
//! An effect is a stored, numbered waveform that modulates one attribute of the
//! fixtures it is applied to. Effects run as a **live layer on top of the
//! programmed level**: the base is the programmer over the running playback, the
//! waveform adds an offset in −size..=+size, and the result is clamped to 0..100%
//! before the intensity masters are applied. Nothing is written into the
//! programmer, cues or palettes, and running assignments are not saved with the
//! show, so load starts with every effect stopped.
//!
//! The waveform is a pure function of time, so the engine tick can evaluate it
//! without sleeping and tests can drive it with explicit instants.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::show::Show;

/// Fastest effect accepted, in cycles per second.
pub const MAX_RATE_HZ: f64 = 100.0;
/// Largest phase spread accepted, in degrees.
pub const MAX_SPREAD_DEGREES: f64 = 360.0;
pub const MAX_EFFECTS: usize = 4096;

fn default_attribute() -> String {
    "Dimmer".into()
}

fn default_rate() -> f64 {
    1.0
}

fn default_size() -> f64 {
    0.5
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Waveform {
    #[default]
    Sine,
    Square,
    Triangle,
    Ramp,
}

impl Waveform {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "sine" => Some(Self::Sine),
            "square" => Some(Self::Square),
            "triangle" => Some(Self::Triangle),
            "ramp" => Some(Self::Ramp),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sine => "Sine",
            Self::Square => "Square",
            Self::Triangle => "Triangle",
            Self::Ramp => "Ramp",
        }
    }

    pub fn names() -> &'static str {
        "Sine, Square, Triangle or Ramp"
    }

    /// One cycle of the wave over phase 0..1, in the range −1..=1.
    pub fn value(self, phase: f64) -> f64 {
        let phase = phase.rem_euclid(1.0);
        match self {
            Self::Sine => (phase * std::f64::consts::TAU).sin(),
            Self::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            Self::Ramp => -1.0 + 2.0 * phase,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    pub number: u16,
    #[serde(default)]
    pub name: String,
    /// Attribute to modulate, e.g. `Dimmer` or `ColorAdd_R`.
    #[serde(default = "default_attribute")]
    pub attribute: String,
    #[serde(default)]
    pub waveform: Waveform,
    /// Cycles per second at a 100% rate master.
    #[serde(default = "default_rate")]
    pub rate_hz: f64,
    /// Amplitude around the base level, 0..=1.
    #[serde(default = "default_size")]
    pub size: f64,
    /// Phase offset between consecutive fixtures, in degrees.
    #[serde(default)]
    pub spread: f64,
}

impl Effect {
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("Effect {}", self.number)
        } else {
            format!("Effect {} \"{}\"", self.number, self.name)
        }
    }
}

/// Process-level effect state: which effect is running on which fixtures, and
/// since when. Not saved with the show.
#[derive(Debug, Default)]
pub struct Effects {
    runs: Vec<Run>,
}

#[derive(Debug)]
struct Run {
    effect: u16,
    fixtures: Vec<u16>,
    started: Instant,
}

impl Effects {
    pub fn is_running(&self) -> bool {
        !self.runs.is_empty()
    }

    /// Starting an effect that is already running replaces its fixture list and
    /// restarts its phase.
    pub fn start(&mut self, effect: u16, fixtures: Vec<u16>, now: Instant) {
        self.runs.retain(|run| run.effect != effect);
        self.runs.push(Run {
            effect,
            fixtures,
            started: now,
        });
    }

    pub fn stop(&mut self, effect: u16) -> bool {
        let before = self.runs.len();
        self.runs.retain(|run| run.effect != effect);
        self.runs.len() != before
    }

    pub fn stop_all(&mut self) -> bool {
        let running = self.is_running();
        self.runs.clear();
        running
    }

    pub fn runs(&self) -> impl Iterator<Item = (u16, &[u16])> {
        self.runs
            .iter()
            .map(|run| (run.effect, run.fixtures.as_slice()))
    }

    /// Current offsets for every running fixture/attribute pair.
    ///
    /// `rate_master` scales every effect's speed; it is a desk fader, not show
    /// data, so it is passed in rather than read from the show.
    pub fn output(&self, show: &Show, rate_master: f64, now: Instant) -> EffectOutput {
        let mut output = EffectOutput::default();
        for run in &self.runs {
            let Some(effect) = show.effect(run.effect) else {
                continue;
            };
            let rate = effect.rate_hz * rate_master;
            let elapsed = now.saturating_duration_since(run.started).as_secs_f64();
            for (index, fixture) in run.fixtures.iter().enumerate() {
                let phase = elapsed.mul_add(rate, index as f64 * effect.spread / 360.0);
                let offset = effect.size * effect.waveform.value(phase);
                output.add(*fixture, &effect.attribute, offset);
            }
        }
        output
    }
}

/// Per-fixture, per-attribute additive offsets produced by running effects.
#[derive(Debug, Default, PartialEq)]
pub struct EffectOutput {
    offsets: BTreeMap<u16, BTreeMap<String, f64>>,
}

impl EffectOutput {
    pub(crate) fn add(&mut self, fixture: u16, attribute: &str, offset: f64) {
        *self
            .offsets
            .entry(fixture)
            .or_default()
            .entry(attribute.to_ascii_lowercase())
            .or_default() += offset;
    }

    /// Attribute names are matched case-insensitively.
    pub fn get(&self, fixture: u16, attribute: &str) -> Option<f64> {
        self.offsets
            .get(&fixture)
            .and_then(|attributes| attributes.get(&attribute.to_ascii_lowercase()))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::demo_show;
    use std::time::Duration;

    fn effect() -> Effect {
        Effect {
            number: 1,
            name: "Pulse".into(),
            attribute: "Dimmer".into(),
            waveform: Waveform::Sine,
            rate_hz: 1.0,
            size: 0.5,
            spread: 0.0,
        }
    }

    #[test]
    fn waveforms_run_one_cycle_from_minus_one_to_one() {
        for waveform in [
            Waveform::Sine,
            Waveform::Square,
            Waveform::Triangle,
            Waveform::Ramp,
        ] {
            for phase in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let value = waveform.value(phase);
                assert!(
                    (-1.0..=1.0).contains(&value),
                    "{waveform:?} at {phase} gave {value}"
                );
            }
        }
        assert!((Waveform::Sine.value(0.25) - 1.0).abs() < 1e-12);
        assert!((Waveform::Square.value(0.4) - 1.0).abs() < 1e-12);
        assert!((Waveform::Square.value(0.6) + 1.0).abs() < 1e-12);
        assert!((Waveform::Triangle.value(0.5) - 1.0).abs() < 1e-12);
        assert!((Waveform::Triangle.value(0.0) + 1.0).abs() < 1e-12);
        assert!((Waveform::Ramp.value(0.0) + 1.0).abs() < 1e-12);
        assert!(Waveform::Ramp.value(0.5).abs() < 1e-12);
    }

    #[test]
    fn an_effect_oscillates_around_the_base_at_its_rate() {
        let mut show = demo_show();
        show.effects.push(effect());
        let start = Instant::now();
        let mut effects = Effects::default();
        effects.start(1, vec![1], start);

        let quarter = effects.output(&show, 1.0, start + Duration::from_millis(250));
        assert!((quarter.get(1, "Dimmer").unwrap() - 0.5).abs() < 1e-9);

        let three_quarter = effects.output(&show, 1.0, start + Duration::from_millis(750));
        assert!((three_quarter.get(1, "Dimmer").unwrap() + 0.5).abs() < 1e-9);

        let cycle = effects.output(&show, 1.0, start + Duration::from_secs(1));
        assert!(cycle.get(1, "Dimmer").unwrap().abs() < 1e-9);
    }

    #[test]
    fn spread_offsets_consecutive_fixtures() {
        let mut show = demo_show();
        show.effects.push(Effect {
            spread: 180.0,
            ..effect()
        });
        let start = Instant::now();
        let mut effects = Effects::default();
        effects.start(1, vec![1, 2], start);

        let output = effects.output(&show, 1.0, start);
        assert!(output.get(1, "Dimmer").unwrap().abs() < 1e-9);
        assert!(output.get(2, "Dimmer").unwrap().abs() < 1e-9);
        let quarter = effects.output(&show, 1.0, start + Duration::from_millis(250));
        assert!((quarter.get(1, "Dimmer").unwrap() - 0.5).abs() < 1e-9);
        assert!((quarter.get(2, "Dimmer").unwrap() + 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_rate_master_scales_the_effect_speed() {
        let mut show = demo_show();
        show.effects.push(effect());
        let start = Instant::now();
        let mut effects = Effects::default();
        effects.start(1, vec![1], start);

        let output = effects.output(&show, 2.0, start + Duration::from_millis(125));
        assert!(
            (output.get(1, "Dimmer").unwrap() - 0.5).abs() < 1e-9,
            "a quarter cycle takes 125 ms at 2 Hz"
        );
    }

    #[test]
    fn restarting_an_effect_replaces_its_run() {
        let start = Instant::now();
        let mut effects = Effects::default();
        effects.start(1, vec![1], start);
        effects.start(1, vec![2], start);
        assert_eq!(effects.runs().count(), 1);
        assert_eq!(effects.runs().next().unwrap().1, [2]);
        assert!(effects.stop(1));
        assert!(!effects.is_running());
    }
}

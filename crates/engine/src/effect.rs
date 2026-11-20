//! Parameter effects (phasers).
//!
//! An effect is a stored, numbered waveform that modulates one or more
//! attributes of the fixtures it is applied to. Effects run as a **live layer on
//! top of the programmed level**: the base is the programmer over the running
//! playback, the waveform adds an offset in −size..=+size, and the result is
//! clamped to 0..100% before the intensity masters are applied. Nothing is
//! written into the programmer, cues or palettes, and running assignments are
//! not saved with the show, so load starts with every effect stopped.
//!
//! An effect has one or more targets, each with its own size and phase. A
//! target's rate is either a free rate in hertz or a beat division evaluated
//! against the desk tempo at render time, so changing `Tempo` retimes every
//! musical effect without editing them.
//!
//! The waveform is a pure function of time, so the engine tick can evaluate it
//! without sleeping and tests can drive it with explicit instants.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Deserializer, Serialize};

use crate::show::Show;

/// Fastest effect accepted, in cycles per second.
pub const MAX_RATE_HZ: f64 = 100.0;
/// Largest phase spread or target phase accepted, in degrees.
pub const MAX_SPREAD_DEGREES: f64 = 360.0;
pub const MAX_EFFECTS: usize = 4096;
/// Slowest and fastest desk tempo accepted, in beats per minute.
pub const MIN_BPM: f64 = 20.0;
pub const MAX_BPM: f64 = 300.0;
/// The tempo a fresh desk starts at.
pub const DEFAULT_BPM: f64 = 120.0;

fn default_size() -> f64 {
    0.5
}

fn default_rate() -> f64 {
    1.0
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

/// One attribute an effect modulates, with its own amplitude and phase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectTarget {
    pub attribute: String,
    /// Amplitude around the base level, 0..=1.
    #[serde(default = "default_size")]
    pub size: f64,
    /// Phase offset in degrees, 0..=360.
    #[serde(default)]
    pub phase: f64,
}

/// A musical beat division. `1/4` is one beat per cycle, `1/2` two beats, and
/// a triplet is 1.5 times the plain division's beats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BeatDivision {
    #[serde(rename = "1/1")]
    Whole,
    #[serde(rename = "1/2")]
    Half,
    #[default]
    #[serde(rename = "1/4")]
    Quarter,
    #[serde(rename = "1/8")]
    Eighth,
    #[serde(rename = "1/16")]
    Sixteenth,
    #[serde(rename = "1/32")]
    ThirtySecond,
    #[serde(rename = "1/2T")]
    HalfTriplet,
    #[serde(rename = "1/4T")]
    QuarterTriplet,
    #[serde(rename = "1/8T")]
    EighthTriplet,
    #[serde(rename = "1/16T")]
    SixteenthTriplet,
}

impl BeatDivision {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_uppercase().as_str() {
            "1/1" => Some(Self::Whole),
            "1/2" => Some(Self::Half),
            "1/4" => Some(Self::Quarter),
            "1/8" => Some(Self::Eighth),
            "1/16" => Some(Self::Sixteenth),
            "1/32" => Some(Self::ThirtySecond),
            "1/2T" => Some(Self::HalfTriplet),
            "1/4T" => Some(Self::QuarterTriplet),
            "1/8T" => Some(Self::EighthTriplet),
            "1/16T" => Some(Self::SixteenthTriplet),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Whole => "1/1",
            Self::Half => "1/2",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::HalfTriplet => "1/2T",
            Self::QuarterTriplet => "1/4T",
            Self::EighthTriplet => "1/8T",
            Self::SixteenthTriplet => "1/16T",
        }
    }

    pub fn names() -> &'static str {
        "1/1, 1/2, 1/4, 1/8, 1/16, 1/32, 1/2T, 1/4T, 1/8T or 1/16T"
    }

    pub const fn is_triplet(self) -> bool {
        matches!(
            self,
            Self::HalfTriplet | Self::QuarterTriplet | Self::EighthTriplet | Self::SixteenthTriplet
        )
    }

    /// Beats one cycle takes. Triplets are 1.5 times the plain division.
    pub const fn beats(self) -> f64 {
        let plain = match self {
            Self::Whole => 4.0,
            Self::Half => 2.0,
            Self::Quarter => 1.0,
            Self::Eighth => 0.5,
            Self::Sixteenth => 0.25,
            Self::ThirtySecond => 0.125,
            Self::HalfTriplet => 2.0,
            Self::QuarterTriplet => 1.0,
            Self::EighthTriplet => 0.5,
            Self::SixteenthTriplet => 0.25,
        };
        if self.is_triplet() {
            plain * 1.5
        } else {
            plain
        }
    }
}

/// An effect's musical rate. `bpm` records the tempo it was chosen at for
/// reference and for a later `Tempo Off`; running effects always follow the
/// desk tempo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tempo {
    pub bpm: f64,
    pub division: BeatDivision,
}

impl Tempo {
    pub fn cycles_per_second(&self, desk_bpm: f64) -> f64 {
        if !desk_bpm.is_finite() || desk_bpm <= 0.0 {
            return 0.0;
        }
        (desk_bpm / 60.0) / self.division.beats()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Effect {
    pub number: u16,
    pub name: String,
    pub waveform: Waveform,
    pub targets: Vec<EffectTarget>,
    /// `Some` means musical: the division is evaluated against the desk tempo.
    /// `None` means `rate_hz` is authoritative.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo: Option<Tempo>,
    pub rate_hz: f64,
    /// Phase offset between consecutive fixtures, in degrees.
    pub spread: f64,
}

/// The persisted shape, which also accepts the v10 `attribute`/`size` pair.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectWire {
    number: u16,
    #[serde(default)]
    name: String,
    #[serde(default)]
    waveform: Waveform,
    #[serde(default)]
    targets: Option<Vec<EffectTarget>>,
    #[serde(default)]
    tempo: Option<Tempo>,
    #[serde(default)]
    rate_hz: Option<f64>,
    #[serde(default)]
    spread: Option<f64>,
    #[serde(default)]
    attribute: Option<String>,
    #[serde(default)]
    size: Option<f64>,
}

impl<'de> Deserialize<'de> for Effect {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = EffectWire::deserialize(deserializer)?;
        let targets = wire.targets.unwrap_or_else(|| {
            // The v10 effect had one attribute and one size.
            vec![EffectTarget {
                attribute: wire.attribute.unwrap_or_default(),
                size: wire.size.unwrap_or_else(default_size),
                phase: 0.0,
            }]
        });
        Ok(Self {
            number: wire.number,
            name: wire.name,
            waveform: wire.waveform,
            targets,
            tempo: wire.tempo,
            rate_hz: wire.rate_hz.unwrap_or_else(default_rate),
            spread: wire.spread.unwrap_or(0.0),
        })
    }
}

impl Effect {
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("Effect {}", self.number)
        } else {
            format!("Effect {} \"{}\"", self.number, self.name)
        }
    }

    /// Cycles per second right now: the desk tempo for a musical effect, else
    /// the stored free rate.
    pub fn effective_rate(&self, desk_tempo: f64) -> f64 {
        match &self.tempo {
            Some(tempo) => tempo.cycles_per_second(desk_tempo),
            None => self.rate_hz,
        }
    }

    /// How the rate reads in a listing or message.
    pub fn rate_label(&self, desk_tempo: f64) -> String {
        match &self.tempo {
            Some(tempo) => format!(
                "{} @ {} BPM",
                tempo.division.label(),
                desk_tempo.round() as i64
            ),
            None => format!("{} Hz", self.rate_hz),
        }
    }

    /// The targets' attribute names, for listings and messages.
    pub fn target_names(&self) -> String {
        self.targets
            .iter()
            .map(|target| target.attribute.as_str())
            .collect::<Vec<_>>()
            .join("+")
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
    /// data. `desk_tempo` is read for musical targets at evaluation time.
    pub fn output(
        &self,
        show: &Show,
        rate_master: f64,
        desk_tempo: f64,
        now: Instant,
    ) -> EffectOutput {
        let mut output = EffectOutput::default();
        for run in &self.runs {
            let Some(effect) = show.effect(run.effect) else {
                continue;
            };
            let rate = effect.effective_rate(desk_tempo) * rate_master;
            let elapsed = now.saturating_duration_since(run.started).as_secs_f64();
            for (index, fixture) in run.fixtures.iter().enumerate() {
                let spread = index as f64 * effect.spread / 360.0;
                for target in &effect.targets {
                    let phase = elapsed.mul_add(rate, spread + target.phase / 360.0);
                    let offset = target.size * effect.waveform.value(phase);
                    output.add(*fixture, &target.attribute, offset);
                }
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

    fn target(attribute: &str, size: f64, phase: f64) -> EffectTarget {
        EffectTarget {
            attribute: attribute.into(),
            size,
            phase,
        }
    }

    fn effect() -> Effect {
        Effect {
            number: 1,
            name: "Pulse".into(),
            waveform: Waveform::Sine,
            targets: vec![target("Dimmer", 0.5, 0.0)],
            tempo: None,
            rate_hz: 1.0,
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
    fn beat_divisions_turn_into_cycles_at_the_desk_tempo() {
        let tempo = |division| Tempo {
            bpm: DEFAULT_BPM,
            division,
        };
        assert!((tempo(BeatDivision::Quarter).cycles_per_second(120.0) - 2.0).abs() < 1e-12);
        assert!((tempo(BeatDivision::Half).cycles_per_second(120.0) - 1.0).abs() < 1e-12);
        assert!((tempo(BeatDivision::Eighth).cycles_per_second(120.0) - 4.0).abs() < 1e-12);
        assert!(
            (tempo(BeatDivision::QuarterTriplet).cycles_per_second(120.0) - 2.0 / 1.5).abs()
                < 1e-12
        );
        assert!((tempo(BeatDivision::Whole).cycles_per_second(120.0) - 0.5).abs() < 1e-12);
        assert_eq!(
            BeatDivision::parse("1/4t"),
            Some(BeatDivision::QuarterTriplet)
        );
        assert_eq!(BeatDivision::parse("1/3"), None);
    }

    #[test]
    fn an_effect_oscillates_around_the_base_at_its_rate() {
        let mut show = demo_show();
        show.effects.push(effect());
        let start = Instant::now();
        let mut effects = Effects::default();
        effects.start(1, vec![1], start);

        let quarter = effects.output(&show, 1.0, 120.0, start + Duration::from_millis(250));
        assert!((quarter.get(1, "Dimmer").unwrap() - 0.5).abs() < 1e-9);

        let three_quarter = effects.output(&show, 1.0, 120.0, start + Duration::from_millis(750));
        assert!((three_quarter.get(1, "Dimmer").unwrap() + 0.5).abs() < 1e-9);

        let cycle = effects.output(&show, 1.0, 120.0, start + Duration::from_secs(1));
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

        let output = effects.output(&show, 1.0, 120.0, start);
        assert!(output.get(1, "Dimmer").unwrap().abs() < 1e-9);
        assert!(output.get(2, "Dimmer").unwrap().abs() < 1e-9);
        let quarter = effects.output(&show, 1.0, 120.0, start + Duration::from_millis(250));
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

        let output = effects.output(&show, 2.0, 120.0, start + Duration::from_millis(125));
        assert!(
            (output.get(1, "Dimmer").unwrap() - 0.5).abs() < 1e-9,
            "a quarter cycle takes 125 ms at 2 Hz"
        );
    }

    #[test]
    fn a_musical_effect_follows_a_tempo_change_without_restarting() {
        let mut show = demo_show();
        show.effects.push(Effect {
            tempo: Some(Tempo {
                bpm: 120.0,
                division: BeatDivision::Quarter,
            }),
            targets: vec![target("Dimmer", 0.5, 0.0)],
            ..effect()
        });
        let start = Instant::now();
        let mut effects = Effects::default();
        effects.start(1, vec![1], start);

        let at_120 = effects.output(&show, 1.0, 120.0, start + Duration::from_millis(125));
        assert!((at_120.get(1, "Dimmer").unwrap() - 0.5).abs() < 1e-9);
        // At half the tempo the same elapsed time is only an eighth of a cycle.
        let at_60 = effects.output(&show, 1.0, 60.0, start + Duration::from_millis(125));
        let expected = 0.5 * (0.125 * std::f64::consts::TAU).sin();
        assert!(
            (at_60.get(1, "Dimmer").unwrap() - expected).abs() < 1e-9,
            "the run kept its start time"
        );
    }

    #[test]
    fn two_targets_with_a_quarter_cycle_phase_difference_are_orthogonal() {
        let mut show = demo_show();
        show.effects.push(Effect {
            targets: vec![target("Pan", 0.5, 0.0), target("Tilt", 0.5, 90.0)],
            ..effect()
        });
        let start = Instant::now();
        let mut effects = Effects::default();
        effects.start(1, vec![1], start);

        for quarter in [1u64, 3, 5, 7] {
            let at = start + Duration::from_millis(250 * quarter);
            let output = effects.output(&show, 1.0, 120.0, at);
            let pan = output.get(1, "Pan").unwrap();
            let tilt = output.get(1, "Tilt").unwrap();
            let expected_pan = 0.5 * (quarter as f64 * 0.25 * std::f64::consts::TAU).sin();
            let expected_tilt =
                0.5 * ((quarter as f64 * 0.25 + 0.25) * std::f64::consts::TAU).sin();
            assert!(
                (pan - expected_pan).abs() < 1e-9 && (tilt - expected_tilt).abs() < 1e-9,
                "at quarter {quarter}: pan {pan}, tilt {tilt}"
            );
        }
    }

    #[test]
    fn a_target_the_fixture_lacks_does_not_disturb_the_others() {
        let mut show = demo_show();
        show.effects.push(Effect {
            targets: vec![target("Dimmer", 0.5, 0.0), target("Nope", 0.4, 0.0)],
            ..effect()
        });
        let start = Instant::now();
        let mut effects = Effects::default();
        effects.start(1, vec![1], start);
        let output = effects.output(&show, 1.0, 120.0, start + Duration::from_millis(250));
        assert!((output.get(1, "Dimmer").unwrap() - 0.5).abs() < 1e-9);
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

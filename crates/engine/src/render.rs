//! Rendering: show state and playback output to one DMX universe frame.
//!
//! Rendering is pure: it reads a validated show, the current playback output
//! and the current effect offsets, and writes 512 slot values per universe. The
//! output thread decides when to call it and which transports receive the
//! result.
//!
//! **Programmer priority.** A value the operator has in the programmer wins
//! over playback, which is what makes editing while a cue runs predictable.
//! `Clear Values` hands the attribute back to playback.
//!
//! **Effects** modulate the merged level and are clamped to 0..100%. They never
//! write the programmer, so clearing an effect leaves the programmed look
//! intact.
//!
//! **Masters** come last and scale intensity only: the grand master times the
//! lowest master of every group that contains the fixture. Colour and position
//! are untouched, so pulling a master down dims without changing the look.

use std::collections::BTreeMap;

use dmx::DMX_SLOTS;

use crate::desk::Masters;
use crate::effect::EffectOutput;
use crate::levels::{is_intensity, Levels};
use crate::show::{Fixture, Show};

/// Where a rendered base value came from. The sheet prints these as `P`, `Pb`,
/// `H` or `-`, which makes an unprogrammed but profile-coloured fixture legible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    Programmer,
    Playback,
    Home,
    None,
}

impl ValueSource {
    pub fn label(self) -> &'static str {
        match self {
            ValueSource::Programmer => "P",
            ValueSource::Playback => "Pb",
            ValueSource::Home => "H",
            ValueSource::None => "-",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputFrame {
    universes: BTreeMap<u16, [u8; DMX_SLOTS]>,
}

impl OutputFrame {
    pub fn universes(&self) -> impl Iterator<Item = (u16, &[u8; DMX_SLOTS])> {
        self.universes
            .iter()
            .map(|(number, slots)| (*number, slots))
    }

    pub fn get(&self, universe: u16) -> Option<&[u8; DMX_SLOTS]> {
        self.universes.get(&universe)
    }

    /// Slot value at a 1-based DMX address.
    pub fn slot(&self, universe: u16, address: u16) -> Option<u8> {
        let slots = self.universes.get(&universe)?;
        slots.get(usize::from(address.checked_sub(1)?)).copied()
    }

    pub fn len(&self) -> usize {
        self.universes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.universes.is_empty()
    }

    fn ensure(&mut self, universe: u16) -> &mut [u8; DMX_SLOTS] {
        self.universes.entry(universe).or_insert([0; DMX_SLOTS])
    }
}

/// Combined master for one fixture: the grand master times the lowest group
/// master of every group that contains it.
pub fn intensity_master(show: &Show, masters: &Masters, fixture: u16) -> f64 {
    let group = show
        .groups
        .iter()
        .filter(|group| group.contains(fixture))
        .map(|group| masters.group(group.number))
        .fold(1.0, f64::min);
    masters.grand * group
}

/// One fixture's output at the current moment: programmer over playback, then
/// effects, then masters. Shared by the renderer and the fixture sheet so both
/// describe the same output.
pub struct Stage<'a> {
    show: &'a Show,
    programmer: &'a Levels,
    playback: &'a Levels,
    effects: &'a EffectOutput,
    masters: &'a Masters,
    blackout: f64,
}

impl<'a> Stage<'a> {
    pub fn new(
        show: &'a Show,
        programmer: &'a Levels,
        playback: &'a Levels,
        effects: &'a EffectOutput,
        masters: &'a Masters,
        blackout: f64,
    ) -> Self {
        Self {
            show,
            programmer,
            playback,
            effects,
            masters,
            blackout,
        }
    }

    /// Where the base value comes from, before effects and masters.
    pub fn source(&self, fixture: &Fixture, attribute: &str) -> ValueSource {
        if self.programmer.get(fixture.number, attribute).is_some() {
            ValueSource::Programmer
        } else if self.playback.get(fixture.number, attribute).is_some() {
            ValueSource::Playback
        } else if self.show.has_home(fixture, attribute) {
            ValueSource::Home
        } else {
            ValueSource::None
        }
    }

    pub fn level(&self, fixture: &Fixture, attribute: &str) -> f64 {
        // Programmer beats playback beats home beats nothing.
        let base = match self.source(fixture, attribute) {
            ValueSource::Programmer => self
                .programmer
                .get(fixture.number, attribute)
                .unwrap_or(0.0),
            ValueSource::Playback => self.playback.get(fixture.number, attribute).unwrap_or(0.0),
            ValueSource::Home => self.show.home_level(fixture, attribute),
            ValueSource::None => 0.0,
        };
        let level = match self.effects.get(fixture.number, attribute) {
            Some(offset) => (base + offset).clamp(0.0, 1.0),
            None => base,
        };
        if is_intensity(attribute) {
            level * intensity_master(self.show, self.masters, fixture.number) * self.blackout
        } else {
            level
        }
    }
}

/// Render every universe of a show, combining the programmer with playback,
/// effects and masters.
///
/// Fixtures without a resolvable type or mode are skipped: validation rejects
/// them, and a stale show must never take the output thread down.
pub fn render(
    show: &Show,
    programmer: &Levels,
    playback: &Levels,
    effects: &EffectOutput,
    masters: &Masters,
    blackout: f64,
) -> OutputFrame {
    let mut frame = OutputFrame::default();
    for universe in &show.universes {
        frame.ensure(universe.number);
    }

    let stage = Stage::new(show, programmer, playback, effects, masters, blackout);
    for fixture in &show.fixtures {
        let Ok((_, mode)) = show.mode_for(fixture) else {
            continue;
        };
        let Some(slots) = frame.universes.get_mut(&fixture.universe) else {
            continue;
        };
        let base = usize::from(fixture.address.saturating_sub(1));
        for channel in &mode.channels {
            let level = stage.level(fixture, &channel.attribute);
            let raw = channel.raw_from_level(level);
            let offset = base + usize::from(channel.offset.saturating_sub(1));
            if offset < DMX_SLOTS {
                slots[offset] = raw;
            }
        }
    }

    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectOutput;
    use crate::pools::Group;
    use crate::show::Fixture;
    use crate::test_support::demo_show;

    fn masters() -> Masters {
        Masters::new()
    }

    fn program(show: &Show, programmer: &Levels, masters: &Masters) -> OutputFrame {
        render(
            show,
            programmer,
            &Levels::new(),
            &EffectOutput::default(),
            masters,
            1.0,
        )
    }

    fn programmer(show: &Show, values: &[(u16, &str, f64)]) -> Levels {
        let mut levels = Levels::new();
        for (fixture, attribute, level) in values {
            levels.set(*fixture, attribute, *level);
        }
        assert!(show.fixture(1).is_some());
        levels
    }

    #[test]
    fn renders_all_slots_of_each_universe() {
        let frame = program(&demo_show(), &Levels::new(), &masters());
        assert_eq!(frame.len(), 1);
        assert_eq!(frame.get(1).unwrap().len(), DMX_SLOTS);
        assert!(frame.get(1).unwrap().iter().all(|slot| *slot == 0));
    }

    #[test]
    fn maps_attributes_to_patched_addresses() {
        let mut show = demo_show();
        show.fixtures[0].address = 5;
        let programmer = programmer(
            &show,
            &[
                (1, "Dimmer", 1.0),
                (1, "ColorAdd_R", 0.5),
                (1, "ColorAdd_G", 0.0),
                (1, "ColorAdd_B", 0.25),
            ],
        );
        let frame = program(&show, &programmer, &masters());
        assert_eq!(frame.slot(1, 5), Some(255), "dimmer at address 5");
        assert_eq!(frame.slot(1, 6), Some(128), "red at address 6");
        assert_eq!(frame.slot(1, 7), Some(0));
        assert_eq!(frame.slot(1, 8), Some(64));
        assert_eq!(frame.slot(1, 9), Some(0), "unpatched slot stays zero");
        assert_eq!(frame.slot(2, 1), None, "universe 2 is not defined");
    }

    #[test]
    fn unpatched_slots_stay_zero() {
        let show = demo_show();
        let programmer = programmer(&show, &[(1, "Dimmer", 1.0)]);
        let frame = program(&show, &programmer, &masters());
        assert_eq!(frame.slot(1, 1), Some(255));
        assert_eq!(frame.slot(1, 2), Some(0));
        assert_eq!(frame.slot(1, 512), Some(0));
    }

    #[test]
    fn fixtures_with_missing_types_are_skipped_safely() {
        let mut show = demo_show();
        show.fixtures[0].type_key = "Missing".into();
        let frame = program(&show, &Levels::new(), &masters());
        assert_eq!(frame.slot(1, 1), Some(0));
    }

    #[test]
    fn a_fixture_with_no_values_renders_its_home() {
        let mut show = demo_show();
        show.fixtures[0].home.insert("Dimmer".into(), 0.5);
        show.fixtures[0].home.insert("ColorAdd_R".into(), 0.25);
        let frame = program(&show, &Levels::new(), &masters());
        assert_eq!(frame.slot(1, 1), Some(128));
        assert_eq!(frame.slot(1, 2), Some(64));
        assert_eq!(frame.slot(1, 3), Some(0), "no home, no value");
    }

    #[test]
    fn playback_and_programmer_override_home() {
        let mut show = demo_show();
        show.fixtures[0].home.insert("Dimmer".into(), 0.25);
        let mut playback = Levels::new();
        playback.set(1, "Dimmer", 0.5);
        let frame = render(
            &show,
            &Levels::new(),
            &playback,
            &EffectOutput::default(),
            &masters(),
            1.0,
        );
        assert_eq!(frame.slot(1, 1), Some(128), "playback beats home");

        let programmer = programmer(&show, &[(1, "Dimmer", 1.0)]);
        let frame = render(
            &show,
            &programmer,
            &playback,
            &EffectOutput::default(),
            &masters(),
            1.0,
        );
        assert_eq!(frame.slot(1, 1), Some(255), "the programmer beats both");
    }

    #[test]
    fn an_effect_modulates_home() {
        let mut show = demo_show();
        show.fixtures[0].home.insert("Dimmer".into(), 0.5);
        let mut effects = EffectOutput::default();
        effects.add(1, "Dimmer", 0.25);
        let frame = render(
            &show,
            &Levels::new(),
            &Levels::new(),
            &effects,
            &masters(),
            1.0,
        );
        assert_eq!(frame.slot(1, 1), Some(191));
    }

    #[test]
    fn masters_scale_home_intensity_only() {
        let mut show = demo_show();
        show.fixtures[0].home.insert("Dimmer".into(), 1.0);
        show.fixtures[0].home.insert("ColorAdd_R".into(), 1.0);
        let mut masters = masters();
        masters.grand = 0.5;
        let frame = program(&show, &Levels::new(), &masters);
        assert_eq!(frame.slot(1, 1), Some(128));
        assert_eq!(frame.slot(1, 2), Some(255));
    }

    #[test]
    fn the_source_names_programmer_playback_home_or_nothing() {
        let mut show = demo_show();
        show.fixtures[0].home.insert("Dimmer".into(), 0.5);
        let mut programmer = Levels::new();
        programmer.set(1, "Dimmer", 0.4);
        let mut playback = Levels::new();
        playback.set(1, "ColorAdd_R", 0.2);
        let effects = EffectOutput::default();
        let masters = masters();
        let stage = Stage::new(&show, &programmer, &playback, &effects, &masters, 1.0);
        let fixture = &show.fixtures[0];
        assert_eq!(stage.source(fixture, "Dimmer"), ValueSource::Programmer);
        assert_eq!(stage.source(fixture, "ColorAdd_R"), ValueSource::Playback);
        assert_eq!(
            stage.source(fixture, "ColorAdd_G"),
            ValueSource::Home,
            "a profile channel is a home even when its default is zero"
        );
        assert_eq!(stage.source(fixture, "Pan"), ValueSource::None);
    }

    #[test]
    fn playback_drives_fixtures_with_an_empty_programmer() {
        let show = demo_show();
        let mut playback = Levels::new();
        playback.set(1, "Dimmer", 0.5);
        playback.set(1, "ColorAdd_B", 1.0);
        let frame = render(
            &show,
            &Levels::new(),
            &playback,
            &EffectOutput::default(),
            &masters(),
            1.0,
        );
        assert_eq!(frame.slot(1, 1), Some(128));
        assert_eq!(frame.slot(1, 4), Some(255));
    }

    #[test]
    fn programmer_values_win_over_playback() {
        let show = demo_show();
        let programmer = programmer(&show, &[(1, "Dimmer", 1.0)]);
        let mut playback = Levels::new();
        playback.set(1, "Dimmer", 0.25);
        let frame = render(
            &show,
            &programmer,
            &playback,
            &EffectOutput::default(),
            &masters(),
            1.0,
        );
        assert_eq!(
            frame.slot(1, 1),
            Some(255),
            "the programmer holds the value until it is cleared"
        );
    }

    #[test]
    fn the_grand_master_scales_intensity_only() {
        let show = demo_show();
        let programmer = programmer(&show, &[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 1.0)]);
        let mut masters = masters();
        masters.grand = 0.5;
        let frame = program(&show, &programmer, &masters);
        assert_eq!(frame.slot(1, 1), Some(128), "dimmer is halved");
        assert_eq!(frame.slot(1, 2), Some(255), "colour is untouched");
    }

    #[test]
    fn a_group_master_scales_its_members_only() {
        let mut show = demo_show();
        show.fixtures.push(Fixture {
            number: 2,
            name: "Second".into(),
            type_key: "OpenLight Demo Wash".into(),
            mode: "4ch".into(),
            universe: 1,
            address: 5,
            home: Default::default(),
        });
        show.groups.push(Group {
            number: 1,
            name: "Front".into(),
            fixtures: vec![1],
        });
        let mut masters = masters();
        masters.set_group(1, 0.5);
        let programmer = programmer(&show, &[(1, "Dimmer", 1.0), (2, "Dimmer", 1.0)]);
        let frame = program(&show, &programmer, &masters);
        assert_eq!(frame.slot(1, 1), Some(128), "fixture 1 is in group 1");
        assert_eq!(frame.slot(1, 5), Some(255), "fixture 2 is not");
    }

    #[test]
    fn overlapping_groups_use_the_lowest_master() {
        let mut show = demo_show();
        for number in [1, 2] {
            show.groups.push(Group {
                number,
                name: String::new(),
                fixtures: vec![1],
            });
        }
        let mut masters = masters();
        masters.set_group(1, 0.5);
        masters.set_group(2, 0.25);
        assert_eq!(intensity_master(&show, &masters, 1), 0.25);
    }

    #[test]
    fn effects_modulate_the_level_and_clamp() {
        let show = demo_show();
        let programmer = programmer(&show, &[(1, "Dimmer", 0.6)]);
        let mut effects = EffectOutput::default();
        effects.add(1, "Dimmer", 0.2);
        let frame = render(
            &show,
            &programmer,
            &Levels::new(),
            &effects,
            &masters(),
            1.0,
        );
        assert_eq!(frame.slot(1, 1), Some(204));

        let mut clipped = EffectOutput::default();
        clipped.add(1, "Dimmer", 5.0);
        let frame = render(
            &show,
            &programmer,
            &Levels::new(),
            &clipped,
            &masters(),
            1.0,
        );
        assert_eq!(frame.slot(1, 1), Some(255), "clamped to full");
    }

    #[test]
    fn masters_do_not_scale_effect_colour() {
        let show = demo_show();
        let programmer = programmer(&show, &[(1, "ColorAdd_R", 1.0)]);
        let mut masters = masters();
        masters.grand = 0.5;
        let mut effects = EffectOutput::default();
        effects.add(1, "ColorAdd_R", -0.5);
        let frame = render(&show, &programmer, &Levels::new(), &effects, &masters, 1.0);
        assert_eq!(frame.slot(1, 2), Some(128), "half red, no master applied");
    }

    #[test]
    fn multiple_fixtures_and_universes() {
        let mut show = demo_show();
        show.universes.push(crate::show::Universe {
            number: 2,
            label: "Universe 2".into(),
            size: 512,
        });
        show.fixtures.push(Fixture {
            number: 2,
            name: "Second".into(),
            type_key: "OpenLight Demo Wash".into(),
            mode: "4ch".into(),
            universe: 2,
            address: 1,
            home: Default::default(),
        });
        let programmer = programmer(&show, &[(1, "Dimmer", 1.0), (2, "Dimmer", 1.0)]);
        let mut masters = masters();
        masters.grand = 0.0;
        let frame = program(&show, &programmer, &masters);
        assert_eq!(frame.len(), 2);
        assert_eq!(frame.slot(1, 1), Some(0));
        assert_eq!(frame.slot(2, 1), Some(0), "both are under the grand master");

        let frame = program(&show, &programmer, &Masters::new());
        assert_eq!(frame.slot(1, 1), Some(255));
        assert_eq!(frame.slot(2, 1), Some(255));
    }

    #[test]
    fn blackout_zeroes_intensity_and_keeps_colour() {
        let show = demo_show();
        let programmer = programmer(&show, &[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 1.0)]);
        let lit = program(&show, &programmer, &masters());
        let dark = render(
            &show,
            &programmer,
            &Levels::new(),
            &EffectOutput::default(),
            &masters(),
            0.0,
        );
        assert_eq!(dark.slot(1, 1), Some(0), "dimmer is blacked out");
        assert_eq!(dark.slot(1, 2), Some(255), "colour stays where it was");

        // Lifting the blackout restores the exact frame.
        let again = program(&show, &programmer, &masters());
        assert_eq!(again.slot(1, 1), lit.slot(1, 1));
        assert_eq!(again.slot(1, 2), lit.slot(1, 2));
    }

    #[test]
    fn a_half_faded_blackout_is_half_intensity() {
        let show = demo_show();
        let programmer = programmer(&show, &[(1, "Dimmer", 1.0)]);
        let frame = render(
            &show,
            &programmer,
            &Levels::new(),
            &EffectOutput::default(),
            &masters(),
            0.5,
        );
        assert_eq!(frame.slot(1, 1), Some(128));
    }
}

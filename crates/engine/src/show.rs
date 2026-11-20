//! The show file.
//!
//! A show is self-contained: fixture types are embedded, so a show opened on
//! another machine never depends on a local fixture library. The format is
//! versioned and every document is validated on load and before save.
//!
//! Version 2 adds palettes, sequences and executors. Version 3 adds groups,
//! worlds, filters and effects. Version 4 adds timecode shows. Version 5 moves
//! the programmer, the masters and the executor faders out of the show and into
//! the desk file, because they are handles rather than programming. Version 6
//! adds cue triggers and executor modes. Version 7 adds per-attribute timing,
//! curves and stagger. Version 8 adds each fixture's home look, the base that
//! renders when neither the programmer nor playback has a value. Version 9 adds
//! cue `blocked` and `shield`, the two ways an operator stops tracking from
//! rippling through the rest of the list. Older files still load: new sections
//! default to empty, the v5 upgrade hands the values it finds in an old file to
//! the desk through a [`LoadReport`], and the v8 upgrade reports any fixture
//! whose profile default is not zero.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::effect::{Effect, MAX_EFFECTS, MAX_RATE_HZ, MAX_SPREAD_DEGREES};
use crate::fixture_type::{FixtureType, Mode};
use crate::levels::Levels;
use crate::palette::Palette;
use crate::patch::{self, PatchRange};
use crate::playback::{format_cue_number, Executor, Sequence, MAX_CUE_NUMBER, MAX_TIME};
use crate::pools::{Filter, Group, World, MAX_GROUP_FIXTURES, MAX_POOL_OBJECTS};
use crate::timecode::{self, TimecodeShow, MAX_FPS, MAX_TIMECODES, MAX_TIMECODE_EVENTS, MIN_FPS};

pub const SHOW_FORMAT: &str = "openlight-show";
pub const SHOW_VERSION: u32 = 9;
/// Older documents that still load, oldest first.
pub const READABLE_VERSIONS: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
/// Fastest rate master accepted, as a factor (400%).
pub const MAX_RATE_MASTER: f64 = 4.0;
pub const MAX_UNIVERSE_SIZE: u16 = 512;
pub const MAX_UNIVERSE_NUMBER: u16 = 63_999;
pub const MAX_FIXTURES: usize = 4096;
pub const MAX_PALETTES: usize = 100_000;
pub const MAX_SEQUENCES: usize = 4096;
pub const MAX_CUES: usize = 65_536;
pub const MAX_EXECUTORS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Show {
    pub format: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub fixture_types: Vec<FixtureType>,
    #[serde(default)]
    pub universes: Vec<Universe>,
    #[serde(default)]
    pub fixtures: Vec<Fixture>,
    #[serde(default)]
    pub palettes: Vec<Palette>,
    #[serde(default)]
    pub sequences: Vec<Sequence>,
    #[serde(default)]
    pub executors: Vec<Executor>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub worlds: Vec<World>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub timecodes: Vec<TimecodeShow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Universe {
    pub number: u16,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_universe_size")]
    pub size: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub number: u16,
    #[serde(default)]
    pub name: String,
    pub type_key: String,
    pub mode: String,
    pub universe: u16,
    pub address: u16,
    /// Base look: attribute → normalised level. Missing attributes fall back to
    /// the profile's channel default, so an edited home only stores overrides.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub home: BTreeMap<String, f64>,
}

const fn default_universe_size() -> u16 {
    MAX_UNIVERSE_SIZE
}

impl Default for Show {
    fn default() -> Self {
        Self::new()
    }
}

impl Show {
    pub fn new() -> Self {
        Self {
            format: SHOW_FORMAT.to_string(),
            version: SHOW_VERSION,
            name: "Untitled Show".into(),
            fixture_types: Vec::new(),
            universes: vec![Universe {
                number: 1,
                label: "Universe 1".into(),
                size: MAX_UNIVERSE_SIZE,
            }],
            fixtures: Vec::new(),
            palettes: Vec::new(),
            sequences: Vec::new(),
            executors: Vec::new(),
            groups: Vec::new(),
            worlds: Vec::new(),
            filters: Vec::new(),
            effects: Vec::new(),
            timecodes: Vec::new(),
        }
    }

    pub fn fixture(&self, number: u16) -> Option<&Fixture> {
        self.fixture_index(number)
            .map(|index| &self.fixtures[index])
    }

    pub fn fixture_index(&self, number: u16) -> Option<usize> {
        self.fixtures
            .iter()
            .position(|fixture| fixture.number == number)
    }

    pub fn universe(&self, number: u16) -> Option<&Universe> {
        self.universes
            .iter()
            .find(|universe| universe.number == number)
    }

    pub fn palette(&self, feature: &str, number: u16) -> Option<&Palette> {
        self.palettes.iter().find(|palette| {
            palette.number == number && palette.feature.eq_ignore_ascii_case(feature.trim())
        })
    }

    pub fn sequence(&self, number: u16) -> Option<&Sequence> {
        self.sequences
            .iter()
            .find(|sequence| sequence.number == number)
    }

    pub fn sequence_index(&self, number: u16) -> Option<usize> {
        self.sequences
            .iter()
            .position(|sequence| sequence.number == number)
    }

    pub fn executor(&self, number: u16) -> Option<&Executor> {
        self.executors
            .iter()
            .find(|executor| executor.number == number)
    }

    pub fn group(&self, number: u16) -> Option<&Group> {
        self.groups.iter().find(|group| group.number == number)
    }

    pub fn world(&self, number: u16) -> Option<&World> {
        self.worlds.iter().find(|world| world.number == number)
    }

    pub fn filter(&self, number: u16) -> Option<&Filter> {
        self.filters.iter().find(|filter| filter.number == number)
    }

    pub fn effect(&self, number: u16) -> Option<&Effect> {
        self.effects.iter().find(|effect| effect.number == number)
    }

    pub fn timecode(&self, number: u16) -> Option<&TimecodeShow> {
        self.timecodes
            .iter()
            .find(|timecode| timecode.number == number)
    }

    pub fn find_type(&self, key: &str) -> Option<&FixtureType> {
        self.fixture_types
            .iter()
            .find(|fixture_type| fixture_type.key().eq_ignore_ascii_case(key.trim()))
    }

    /// A fixture's home level for one attribute: the stored override, else the
    /// channel's GDTF default, else zero.
    pub fn home_level(&self, fixture: &Fixture, attribute: &str) -> f64 {
        if let Some((_, level)) = fixture
            .home
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(attribute))
        {
            return *level;
        }
        let Ok((_, mode)) = self.mode_for(fixture) else {
            return 0.0;
        };
        mode.channel_for(attribute)
            .map(|channel| channel.level_from_raw(channel.default_raw))
            .unwrap_or(0.0)
    }

    /// Whether a home exists for this attribute, even when it is zero.
    pub fn has_home(&self, fixture: &Fixture, attribute: &str) -> bool {
        fixture
            .home
            .keys()
            .any(|name| name.eq_ignore_ascii_case(attribute))
            || self
                .mode_for(fixture)
                .map(|(_, mode)| mode.channel_for(attribute).is_some())
                .unwrap_or(false)
    }

    /// Resolve the fixture type and mode of a patched fixture.
    pub fn mode_for(&self, fixture: &Fixture) -> Result<(&FixtureType, &Mode), String> {
        let fixture_type = self
            .find_type(&fixture.type_key)
            .ok_or_else(|| format!("fixture type `{}` is not in the show", fixture.type_key))?;
        let mode = fixture_type.mode(&fixture.mode).ok_or_else(|| {
            format!(
                "mode `{}` is not in fixture type `{}`",
                fixture.mode,
                fixture_type.key()
            )
        })?;
        Ok((fixture_type, mode))
    }

    /// Every problem that would make this show unsafe to output or save.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();

        if self.format != SHOW_FORMAT {
            problems.push(format!(
                "format is `{}`, expected `{SHOW_FORMAT}`",
                self.format
            ));
        }
        if self.version != SHOW_VERSION {
            problems.push(format!(
                "version is {}, this build reads and writes {SHOW_VERSION}",
                self.version
            ));
        }
        if self.name.trim().is_empty() {
            problems.push("show name is empty".into());
        }

        let mut universe_numbers = BTreeSet::new();
        let mut universe_sizes = BTreeMap::new();
        for universe in &self.universes {
            if universe.number == 0 || universe.number > MAX_UNIVERSE_NUMBER {
                problems.push(format!(
                    "universe {}: number must be 1..={MAX_UNIVERSE_NUMBER}",
                    universe.number
                ));
            }
            if universe.size == 0 || universe.size > MAX_UNIVERSE_SIZE {
                problems.push(format!(
                    "universe {}: size {} must be 1..={MAX_UNIVERSE_SIZE}",
                    universe.number, universe.size
                ));
            }
            if !universe_numbers.insert(universe.number) {
                problems.push(format!("universe {}: defined twice", universe.number));
            }
            universe_sizes.insert(universe.number, universe.size);
        }

        let mut type_keys = BTreeSet::new();
        for fixture_type in &self.fixture_types {
            let key = fixture_type.key();
            if key.is_empty() {
                problems.push("fixture type with empty manufacturer and name".into());
            }
            if !type_keys.insert(key.to_ascii_lowercase()) {
                problems.push(format!("fixture type `{key}`: defined twice"));
            }
            if fixture_type.modes.is_empty() {
                problems.push(format!("fixture type `{key}`: no DMX modes"));
            }
            let mut mode_names = BTreeSet::new();
            for mode in &fixture_type.modes {
                if mode.name.trim().is_empty() {
                    problems.push(format!("fixture type `{key}`: mode without a name"));
                }
                if !mode_names.insert(mode.name.to_ascii_lowercase()) {
                    problems.push(format!(
                        "fixture type `{key}`: mode `{}` defined twice",
                        mode.name
                    ));
                }
                if mode.channels.is_empty() {
                    problems.push(format!(
                        "fixture type `{key}`, mode `{}`: no DMX channels",
                        mode.name
                    ));
                }
                let mut offsets = BTreeSet::new();
                for channel in &mode.channels {
                    if channel.offset == 0 || channel.offset > MAX_UNIVERSE_SIZE {
                        problems.push(format!(
                            "fixture type `{key}`, mode `{}`: channel offset {} out of range",
                            mode.name, channel.offset
                        ));
                    }
                    if !offsets.insert(channel.offset) {
                        problems.push(format!(
                            "fixture type `{key}`, mode `{}`: offset {} used by two channels",
                            mode.name, channel.offset
                        ));
                    }
                    if channel.attribute.trim().is_empty() {
                        problems.push(format!(
                            "fixture type `{key}`, mode `{}`: channel without an attribute",
                            mode.name
                        ));
                    }
                    for function in &channel.functions {
                        if function.dmx_to < function.dmx_from {
                            problems.push(format!(
                                "fixture type `{key}`, mode `{}`: function `{}` has DMX {}..{}",
                                mode.name, function.name, function.dmx_from, function.dmx_to
                            ));
                        }
                        if !function.physical_from.is_finite() || !function.physical_to.is_finite()
                        {
                            problems.push(format!(
                                "fixture type `{key}`, mode `{}`: function `{}` has a non-finite physical range",
                                mode.name, function.name
                            ));
                        }
                    }
                }
            }
        }

        if self.fixtures.len() > MAX_FIXTURES {
            problems.push(format!(
                "{} fixtures patched, the limit is {MAX_FIXTURES}",
                self.fixtures.len()
            ));
        }

        let mut fixture_numbers = BTreeSet::new();
        let mut ranges: Vec<(u16, PatchRange)> = Vec::new();
        for fixture in &self.fixtures {
            if fixture.number == 0 {
                problems.push("fixture number 0 is not allowed".into());
            }
            if !fixture_numbers.insert(fixture.number) {
                problems.push(format!("fixture {}: number used twice", fixture.number));
            }
            for (attribute, level) in &fixture.home {
                if attribute.trim().is_empty() {
                    problems.push(format!(
                        "fixture {}: home value without an attribute",
                        fixture.number
                    ));
                }
                if !level.is_finite() || !(0.0..=1.0).contains(level) {
                    problems.push(format!(
                        "fixture {}: home `{attribute}` is {level}, expected 0..=1",
                        fixture.number
                    ));
                }
            }
            let Some(&universe_size) = universe_sizes.get(&fixture.universe) else {
                problems.push(format!(
                    "fixture {}: universe {} is not defined",
                    fixture.number, fixture.universe
                ));
                continue;
            };
            let Some(fixture_type) = self.find_type(&fixture.type_key) else {
                problems.push(format!(
                    "fixture {}: fixture type `{}` is not in the show",
                    fixture.number, fixture.type_key
                ));
                continue;
            };
            let Some(mode) = fixture_type.mode(&fixture.mode) else {
                problems.push(format!(
                    "fixture {}: mode `{}` is not in fixture type `{}`",
                    fixture.number,
                    fixture.mode,
                    fixture_type.key()
                ));
                continue;
            };
            let Some(range) = PatchRange::new(fixture.universe, fixture.address, mode.footprint())
            else {
                problems.push(format!(
                    "fixture {}: invalid patch address {}",
                    fixture.number, fixture.address
                ));
                continue;
            };
            if !range.fits(universe_size) {
                problems.push(format!(
                    "fixture {}: {} channels at {}.{:03} overrun universe {} ({} slots)",
                    fixture.number,
                    range.footprint(),
                    fixture.universe,
                    fixture.address,
                    fixture.universe,
                    universe_size
                ));
            }
            ranges.push((fixture.number, range));
        }

        for (first, second) in patch::conflicts(&ranges) {
            problems.push(format!(
                "fixtures {first} and {second}: overlapping DMX addresses"
            ));
        }

        self.validate_palettes(&mut problems);
        self.validate_sequences(&mut problems);
        self.validate_executors(&mut problems);
        self.validate_groups(&mut problems);
        self.validate_worlds(&mut problems);
        self.validate_filters(&mut problems);
        self.validate_effects(&mut problems);
        self.validate_timecodes(&mut problems);

        problems
    }

    fn validate_palettes(&self, problems: &mut Vec<String>) {
        if self.palettes.len() > MAX_PALETTES {
            problems.push(format!(
                "{} palettes, the limit is {MAX_PALETTES}",
                self.palettes.len()
            ));
        }
        let mut keys = BTreeSet::new();
        for palette in &self.palettes {
            if palette.feature.trim().is_empty() {
                problems.push(format!("palette {}: feature pool is empty", palette.number));
            }
            if palette.number == 0 {
                problems.push(format!(
                    "palette {} ({}): number 0 is not allowed",
                    palette.label(),
                    palette.feature
                ));
            }
            if !keys.insert((palette.feature.to_ascii_lowercase(), palette.number)) {
                problems.push(format!("palette {}: defined twice", palette.label()));
            }
            for (type_key, values) in &palette.values {
                if self.find_type(type_key).is_none() {
                    problems.push(format!(
                        "palette {}: fixture type `{type_key}` is not in the show",
                        palette.label()
                    ));
                }
                if values.is_empty() {
                    problems.push(format!(
                        "palette {}, type `{type_key}`: no values",
                        palette.label()
                    ));
                }
                for (attribute, level) in values {
                    if attribute.trim().is_empty() {
                        problems.push(format!(
                            "palette {}: value without an attribute",
                            palette.label()
                        ));
                    }
                    if !level.is_finite() || !(0.0..=1.0).contains(level) {
                        problems.push(format!(
                            "palette {}, attribute `{attribute}`: level {level} is outside 0..=1",
                            palette.label()
                        ));
                    }
                }
            }
        }
    }

    fn validate_sequences(&self, problems: &mut Vec<String>) {
        if self.sequences.len() > MAX_SEQUENCES {
            problems.push(format!(
                "{} sequences, the limit is {MAX_SEQUENCES}",
                self.sequences.len()
            ));
        }
        let mut numbers = BTreeSet::new();
        for sequence in &self.sequences {
            if sequence.number == 0 {
                problems.push("sequence number 0 is not allowed".into());
            }
            if !numbers.insert(sequence.number) {
                problems.push(format!("sequence {}: number used twice", sequence.number));
            }
            if sequence.cues.len() > MAX_CUES {
                problems.push(format!(
                    "sequence {}: {} cues, the limit is {MAX_CUES}",
                    sequence.number,
                    sequence.cues.len()
                ));
            }
            // The state a cue leaves behind, so a shield can be checked against
            // the attributes the cue actually has. Kept in step with
            // `Sequence::rebase`, which is what makes a stored shield hold.
            let mut state = Levels::new();
            for cue in &sequence.cues {
                let label = format_cue_number(cue.number);
                if !cue.number.is_finite() || !(0.0..=MAX_CUE_NUMBER).contains(&cue.number) {
                    problems.push(format!(
                        "sequence {}: cue number {label} is outside 0..={MAX_CUE_NUMBER}",
                        sequence.number
                    ));
                }
                for (name, value) in [
                    ("fade", cue.fade),
                    ("delay", cue.delay),
                    ("trigger time", cue.trigger_time),
                    ("stagger", cue.stagger),
                ] {
                    if !value.is_finite() || !(0.0..=MAX_TIME).contains(&value) {
                        problems.push(format!(
                            "sequence {} cue {label}: {name} {value} is outside 0..={MAX_TIME} seconds",
                            sequence.number
                        ));
                    }
                }
                for (fixture, attributes) in cue.levels.fixtures() {
                    for (attribute, level) in attributes {
                        if attribute.trim().is_empty() {
                            problems.push(format!(
                                "sequence {} cue {label}: value without an attribute",
                                sequence.number
                            ));
                        }
                        if !level.is_finite() || !(0.0..=1.0).contains(level) {
                            problems.push(format!(
                                "sequence {} cue {label}: fixture {fixture} `{attribute}` is {level}, expected 0..=1",
                                sequence.number
                            ));
                        }
                    }
                }
                for (attribute, timing) in &cue.timing {
                    if attribute.trim().is_empty() {
                        problems.push(format!(
                            "sequence {} cue {label}: timing without an attribute",
                            sequence.number
                        ));
                    }
                    for (name, value) in [("fade", timing.fade), ("delay", timing.delay)] {
                        if let Some(value) = value {
                            if !value.is_finite() || !(0.0..=MAX_TIME).contains(&value) {
                                problems.push(format!(
                                    "sequence {} cue {label}: `{attribute}` timing {name} {value} is outside 0..={MAX_TIME} seconds",
                                    sequence.number
                                ));
                            }
                        }
                    }
                }
                // A cue with values can only time attributes its own fixture
                // modes have, so a typo is caught at save rather than silently
                // never applying. A cue with no values may carry any timing; it
                // takes effect the next time that attribute appears.
                let mut types = Vec::new();
                for (fixture, _) in cue.levels.fixtures() {
                    if let Some(fixture) = self.fixture(fixture) {
                        if let Ok((fixture_type, _)) = self.mode_for(fixture) {
                            types.push(fixture_type);
                        }
                    }
                }
                for attribute in cue.timing.keys() {
                    if !types.is_empty()
                        && !types
                            .iter()
                            .any(|fixture_type| fixture_type.attribute(attribute).is_some())
                    {
                        problems.push(format!(
                            "sequence {} cue {label}: `{attribute}` is not an attribute of the cue's fixtures",
                            sequence.number
                        ));
                    }
                }

                // Blocked cues store the whole state; tracking cues store a
                // delta. A shield is an assertion, so its attribute must exist
                // in the state the cue leaves behind; rebase applies the level
                // to every fixture that has it.
                let mut absolute = if cue.blocked {
                    cue.levels.clone()
                } else {
                    let mut absolute = state.clone();
                    absolute.overlay(&cue.levels);
                    absolute
                };
                let mut shield_names = BTreeSet::new();
                for (attribute, level) in &cue.shield {
                    if attribute.trim().is_empty() {
                        problems.push(format!(
                            "sequence {} cue {label}: shield without an attribute",
                            sequence.number
                        ));
                    }
                    if !shield_names.insert(attribute.trim().to_ascii_lowercase()) {
                        problems.push(format!(
                            "sequence {} cue {label}: shield `{attribute}` is listed twice",
                            sequence.number
                        ));
                    }
                    if !level.is_finite() || !(0.0..=1.0).contains(level) {
                        problems.push(format!(
                            "sequence {} cue {label}: shield `{attribute}` is {level}, expected 0..=1",
                            sequence.number
                        ));
                    }
                    let known = absolute.fixtures().any(|(_, attributes)| {
                        attributes
                            .keys()
                            .any(|name| name.eq_ignore_ascii_case(attribute))
                    });
                    if !known {
                        problems.push(format!(
                            "sequence {} cue {label}: shield `{attribute}` is not an attribute of the cue's fixtures",
                            sequence.number
                        ));
                    }
                }
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
                state = absolute;
            }
            // Playback advances through the list in order, so a cue that is out
            // of order or repeated is rejected here, not discovered mid-show.
            // Cues are sorted, which makes this an O(n) adjacent check.
            for pair in sequence.cues.windows(2) {
                let (previous, next) = (&pair[0], &pair[1]);
                if (next.number - previous.number).abs() < 1e-9 {
                    problems.push(format!(
                        "sequence {}: cue {} is defined twice",
                        sequence.number,
                        format_cue_number(next.number)
                    ));
                } else if next.number < previous.number {
                    problems.push(format!(
                        "sequence {}: cue {} comes after cue {} and is not in ascending order",
                        sequence.number,
                        format_cue_number(next.number),
                        format_cue_number(previous.number)
                    ));
                }
            }
        }
    }

    fn validate_executors(&self, problems: &mut Vec<String>) {
        if self.executors.len() > MAX_EXECUTORS {
            problems.push(format!(
                "{} executors, the limit is {MAX_EXECUTORS}",
                self.executors.len()
            ));
        }
        let mut numbers = BTreeSet::new();
        for executor in &self.executors {
            if executor.number == 0 {
                problems.push("executor number 0 is not allowed".into());
            }
            if !numbers.insert(executor.number) {
                problems.push(format!("executor {}: number used twice", executor.number));
            }
            if self.sequence(executor.sequence).is_none() {
                problems.push(format!(
                    "executor {}: sequence {} is not in the show",
                    executor.number, executor.sequence
                ));
            }
        }
    }

    fn validate_groups(&self, problems: &mut Vec<String>) {
        if self.groups.len() > MAX_POOL_OBJECTS {
            problems.push(format!(
                "{} groups, the limit is {MAX_POOL_OBJECTS}",
                self.groups.len()
            ));
        }
        let mut numbers = BTreeSet::new();
        for group in &self.groups {
            if group.number == 0 {
                problems.push("group number 0 is not allowed".into());
            }
            if !numbers.insert(group.number) {
                problems.push(format!("group {}: number used twice", group.number));
            }
            if group.fixtures.len() > MAX_GROUP_FIXTURES {
                problems.push(format!(
                    "group {}: {} fixtures, the limit is {MAX_GROUP_FIXTURES}",
                    group.number,
                    group.fixtures.len()
                ));
            }
            let mut members = BTreeSet::new();
            for fixture in &group.fixtures {
                if *fixture == 0 {
                    problems.push(format!("group {}: fixture 0 is not allowed", group.number));
                }
                if !members.insert(*fixture) {
                    problems.push(format!(
                        "group {}: fixture {fixture} is listed twice",
                        group.number
                    ));
                }
            }
        }
    }

    fn validate_worlds(&self, problems: &mut Vec<String>) {
        if self.worlds.len() > MAX_POOL_OBJECTS {
            problems.push(format!(
                "{} worlds, the limit is {MAX_POOL_OBJECTS}",
                self.worlds.len()
            ));
        }
        let mut numbers = BTreeSet::new();
        for world in &self.worlds {
            if world.number == 0 {
                problems.push("world number 0 is not allowed".into());
            }
            if !numbers.insert(world.number) {
                problems.push(format!("world {}: number used twice", world.number));
            }
            if world.fixtures.is_empty() {
                problems.push(format!("world {}: no fixtures", world.number));
            }
            if world.fixtures.len() > MAX_GROUP_FIXTURES {
                problems.push(format!(
                    "world {}: {} fixtures, the limit is {MAX_GROUP_FIXTURES}",
                    world.number,
                    world.fixtures.len()
                ));
            }
            if world.fixtures.contains(&0) {
                problems.push(format!("world {}: fixture 0 is not allowed", world.number));
            }
        }
    }

    fn validate_filters(&self, problems: &mut Vec<String>) {
        if self.filters.len() > MAX_POOL_OBJECTS {
            problems.push(format!(
                "{} filters, the limit is {MAX_POOL_OBJECTS}",
                self.filters.len()
            ));
        }
        let mut numbers = BTreeSet::new();
        for filter in &self.filters {
            if filter.number == 0 {
                problems.push("filter number 0 is not allowed".into());
            }
            if !numbers.insert(filter.number) {
                problems.push(format!("filter {}: number used twice", filter.number));
            }
            if filter.features.is_empty() {
                problems.push(format!("filter {}: no features", filter.number));
            }
            let mut features = BTreeSet::new();
            for feature in &filter.features {
                if feature.trim().is_empty() {
                    problems.push(format!("filter {}: empty feature name", filter.number));
                }
                if !features.insert(feature.trim().to_ascii_lowercase()) {
                    problems.push(format!(
                        "filter {}: feature `{feature}` is listed twice",
                        filter.number
                    ));
                }
            }
        }
    }

    fn validate_effects(&self, problems: &mut Vec<String>) {
        if self.effects.len() > MAX_EFFECTS {
            problems.push(format!(
                "{} effects, the limit is {MAX_EFFECTS}",
                self.effects.len()
            ));
        }
        let mut numbers = BTreeSet::new();
        for effect in &self.effects {
            if effect.number == 0 {
                problems.push("effect number 0 is not allowed".into());
            }
            if !numbers.insert(effect.number) {
                problems.push(format!("effect {}: number used twice", effect.number));
            }
            if effect.attribute.trim().is_empty() {
                problems.push(format!("effect {}: no attribute", effect.number));
            }
            if !effect.rate_hz.is_finite() || !(0.0..=MAX_RATE_HZ).contains(&effect.rate_hz) {
                problems.push(format!(
                    "effect {}: rate {} Hz is outside 0..={MAX_RATE_HZ}",
                    effect.number, effect.rate_hz
                ));
            }
            if !effect.size.is_finite() || !(0.0..=1.0).contains(&effect.size) {
                problems.push(format!(
                    "effect {}: size {} is outside 0..=1",
                    effect.number, effect.size
                ));
            }
            if !effect.spread.is_finite() || !(0.0..=MAX_SPREAD_DEGREES).contains(&effect.spread) {
                problems.push(format!(
                    "effect {}: spread {} is outside 0..={MAX_SPREAD_DEGREES} degrees",
                    effect.number, effect.spread
                ));
            }
        }
    }

    fn validate_timecodes(&self, problems: &mut Vec<String>) {
        if self.timecodes.len() > MAX_TIMECODES {
            problems.push(format!(
                "{} timecode shows, the limit is {MAX_TIMECODES}",
                self.timecodes.len()
            ));
        }
        let mut numbers = BTreeSet::new();
        for timecode in &self.timecodes {
            if timecode.number == 0 {
                problems.push("timecode number 0 is not allowed".into());
            }
            if !numbers.insert(timecode.number) {
                problems.push(format!("timecode {}: number used twice", timecode.number));
            }
            if !(MIN_FPS..=MAX_FPS).contains(&timecode.fps) {
                problems.push(format!(
                    "timecode {}: {} fps is outside {MIN_FPS}..={MAX_FPS}",
                    timecode.number, timecode.fps
                ));
            }
            if timecode.events.len() > MAX_TIMECODE_EVENTS {
                problems.push(format!(
                    "timecode {}: {} events, the limit is {MAX_TIMECODE_EVENTS}",
                    timecode.number,
                    timecode.events.len()
                ));
            }
            let fps = timecode.frame_rate();
            let mut seen = BTreeSet::new();
            let mut previous = None;
            for event in &timecode.events {
                let frames = match timecode::parse(&event.at, fps) {
                    Ok(frames) => frames,
                    Err(error) => {
                        problems.push(format!("timecode {}: {error}", timecode.number));
                        continue;
                    }
                };
                if !seen.insert(frames) {
                    problems.push(format!(
                        "timecode {}: two events at {}",
                        timecode.number, event.at
                    ));
                }
                if let Some(previous) = previous {
                    if frames < previous {
                        problems.push(format!(
                            "timecode {}: event {} is not in ascending order",
                            timecode.number, event.at
                        ));
                    }
                }
                previous = Some(frames);
                match self.sequence(event.sequence) {
                    None => problems.push(format!(
                        "timecode {}: event {} references sequence {} which is not in the show",
                        timecode.number, event.at, event.sequence
                    )),
                    Some(sequence) => {
                        if sequence.cue_index(event.cue).is_none() {
                            problems.push(format!(
                                "timecode {}: event {} references cue {} which sequence {} does not have",
                                timecode.number,
                                event.at,
                                format_cue_number(event.cue),
                                sequence.number
                            ));
                        }
                    }
                }
            }
        }
    }

    /// Write the show atomically: full document first, then rename over the target.
    pub fn save(&self, path: &Path) -> Result<(), ShowError> {
        let problems = self.validate();
        if !problems.is_empty() {
            return Err(ShowError::Invalid {
                path: path.to_path_buf(),
                problems,
            });
        }
        let json = serde_json::to_string_pretty(self).map_err(|source| ShowError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        crate::store::write_atomic(path, json.as_bytes()).map_err(|source| ShowError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Read and validate a show. A failed load never leaves a half-built show.
    pub fn load(path: &Path) -> Result<Show, ShowError> {
        Self::load_with_report(path).map(|(show, _)| show)
    }

    /// Read a show, also returning anything the reader upgraded.
    ///
    /// The report exists because some old fields belong to somebody else now: a
    /// v4 programmer and its master positions move to the desk, and the loader
    /// hands them over rather than keeping dead fields in the document.
    pub fn load_with_report(path: &Path) -> Result<(Show, LoadReport), ShowError> {
        let text = fs::read_to_string(path).map_err(|source| ShowError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut value: serde_json::Value =
            serde_json::from_str(&text).map_err(|source| ShowError::Parse {
                path: path.to_path_buf(),
                source,
            })?;

        let format = value
            .get("format")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        if format != SHOW_FORMAT {
            return Err(ShowError::WrongFormat {
                path: path.to_path_buf(),
                found: format,
            });
        }
        let version = value
            .get("version")
            .and_then(|value| value.as_u64())
            .unwrap_or_default() as u32;
        if !READABLE_VERSIONS.contains(&version) {
            return Err(ShowError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: version,
                supported: SHOW_VERSION,
            });
        }

        let mut report = upgrade(&mut value, version, path)?;
        let mut show: Show = serde_json::from_value(value).map_err(|source| ShowError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        // Older versions lack the newer sections. Everything defaults (empty
        // pools, no timecode), so the document is upgraded in place on read and
        // saved as the current version.
        show.version = SHOW_VERSION;
        let problems = show.validate();
        if !problems.is_empty() {
            return Err(ShowError::Invalid {
                path: path.to_path_buf(),
                problems,
            });
        }
        if version < 8 {
            if let Some(warning) = home_upgrade_warning(&show) {
                report.warnings.push(warning);
            }
        }
        Ok((show, report))
    }
}

/// The v8 upgrade note: the base look now comes from the profiles, so say which
/// fixtures move before the operator sees them move.
fn home_upgrade_warning(show: &Show) -> Option<String> {
    let mut moved = Vec::new();
    for fixture in &show.fixtures {
        let Ok((_, mode)) = show.mode_for(fixture) else {
            continue;
        };
        if let Some((channel, level)) = mode.channels.iter().find_map(|channel| {
            let level = show.home_level(fixture, &channel.attribute);
            (level > 0.0).then_some((channel, level))
        }) {
            moved.push(format!(
                "fixture {}: {} {}%",
                fixture.number,
                channel.attribute,
                (level * 100.0).round() as i64
            ));
        }
    }
    if moved.is_empty() {
        return None;
    }
    let examples = moved.join("; ");
    Some(format!(
        "Home seeded from profiles: {} {} a non-zero default ({examples})",
        moved.len(),
        if moved.len() == 1 {
            "fixture has"
        } else {
            "fixtures have"
        },
    ))
}

/// Values an older show carried that now belong to the desk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeskSeed {
    pub programmer: Levels,
    pub grand: Option<f64>,
    pub rate: Option<f64>,
    pub groups: BTreeMap<u16, f64>,
    pub executors: BTreeMap<u16, f64>,
}

impl DeskSeed {
    pub fn is_empty(&self) -> bool {
        self.programmer.is_empty()
            && self.grand.is_none()
            && self.rate.is_none()
            && self.groups.is_empty()
            && self.executors.is_empty()
    }
}

/// What a load had to change, and what it found for the desk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadReport {
    pub warnings: Vec<String>,
    pub desk_seed: DeskSeed,
}

/// Strip fields that moved out of the show, so a v1–v4 document still reads.
fn upgrade(
    value: &mut serde_json::Value,
    version: u32,
    path: &Path,
) -> Result<LoadReport, ShowError> {
    let mut report = LoadReport::default();
    if version >= SHOW_VERSION {
        return Ok(report);
    }

    if let Some(fixtures) = value
        .get_mut("fixtures")
        .and_then(|value| value.as_array_mut())
    {
        for fixture in fixtures {
            let Some(object) = fixture.as_object_mut() else {
                continue;
            };
            let number = object
                .get("number")
                .and_then(|number| number.as_u64())
                .unwrap_or_default() as u16;
            let Some(levels) = object.remove("levels") else {
                continue;
            };
            // v4 stored the programmer as `attribute → level` on each fixture.
            let levels: BTreeMap<String, f64> =
                serde_json::from_value(levels).map_err(|source| ShowError::Parse {
                    path: path.to_path_buf(),
                    source,
                })?;
            for (attribute, level) in levels {
                report.desk_seed.programmer.set(number, &attribute, level);
            }
        }
    }

    if let Some(object) = value.as_object_mut() {
        report.desk_seed.grand = object.remove("grand_master").and_then(|v| v.as_f64());
        report.desk_seed.rate = object.remove("rate_master").and_then(|v| v.as_f64());

        if let Some(groups) = object
            .get_mut("groups")
            .and_then(|value| value.as_array_mut())
        {
            for group in groups {
                let Some(group) = group.as_object_mut() else {
                    continue;
                };
                let number = group
                    .get("number")
                    .and_then(|number| number.as_u64())
                    .unwrap_or_default() as u16;
                if let Some(master) = group.remove("master").and_then(|value| value.as_f64()) {
                    report.desk_seed.groups.insert(number, master);
                }
            }
        }
        if let Some(executors) = object
            .get_mut("executors")
            .and_then(|value| value.as_array_mut())
        {
            for executor in executors {
                let Some(executor) = executor.as_object_mut() else {
                    continue;
                };
                let number = executor
                    .get("number")
                    .and_then(|number| number.as_u64())
                    .unwrap_or_default() as u16;
                if let Some(fader) = executor.remove("fader").and_then(|value| value.as_f64()) {
                    report.desk_seed.executors.insert(number, fader);
                }
            }
        }
    }

    if !report.desk_seed.is_empty() {
        report.warnings.push(format!(
            "Upgraded from show v{version}: programmer and master positions moved to the desk"
        ));
    }
    Ok(report)
}

#[derive(Debug, thiserror::Error)]
pub enum ShowError {
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
    #[error("{}: not an OpenLight show (format `{found}`)", path.display())]
    WrongFormat { path: PathBuf, found: String },
    #[error(
        "{}: show version {found} is not supported (this build reads 1..={supported})",
        path.display()
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("{}: invalid show:\n  {}", path.display(), problems.join("\n  "))]
    Invalid {
        path: PathBuf,
        problems: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{demo_fixture_type, demo_show};
    use std::io::Read;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "openlight-show-{}-{name}.olshow.json",
            std::process::id()
        ))
    }

    #[test]
    fn new_show_is_valid() {
        assert!(Show::new().validate().is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let show = demo_show();
        let path = temp_path("roundtrip");
        show.save(&path).unwrap();
        let loaded = Show::load(&path).unwrap();
        assert_eq!(loaded, show);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn saved_document_is_self_contained_and_readable() {
        let show = demo_show();
        let path = temp_path("self-contained");
        show.save(&path).unwrap();
        let mut text = String::new();
        fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains("\"format\": \"openlight-show\""));
        assert!(text.contains("\"version\": 9"));
        assert!(text.contains("OpenLight Demo Wash"), "types are embedded");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_rejects_future_versions() {
        let path = temp_path("future");
        fs::write(
            &path,
            r#"{"format":"openlight-show","version":10,"name":"x","fixture_types":[],"universes":[],"fixtures":[]}"#,
        )
        .unwrap();
        let error = Show::load(&path).unwrap_err();
        assert!(matches!(
            error,
            ShowError::UnsupportedVersion { found: 10, .. }
        ));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_rejects_unknown_fields() {
        let path = temp_path("unknown-field");
        fs::write(
            &path,
            r#"{"format":"openlight-show","version":1,"name":"x","fixture_types":[],"universes":[],"fixtures":[],"surprise":true}"#,
        )
        .unwrap();
        assert!(matches!(
            Show::load(&path).unwrap_err(),
            ShowError::Parse { .. }
        ));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn overlapping_patch_is_invalid() {
        let mut show = demo_show();
        show.fixtures.push(Fixture {
            number: 2,
            name: "Demo Wash".into(),
            type_key: "OpenLight Demo Wash".into(),
            mode: "4ch".into(),
            universe: 1,
            address: 3,
            home: BTreeMap::new(),
        });
        let problems = show.validate();
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("overlapping")),
            "{problems:?}"
        );
    }

    #[test]
    fn patch_beyond_universe_is_invalid() {
        let mut show = demo_show();
        show.fixtures[0].address = 510;
        let problems = show.validate();
        assert!(
            problems.iter().any(|problem| problem.contains("overrun")),
            "{problems:?}"
        );
    }

    #[test]
    fn unknown_type_and_mode_are_reported() {
        let mut show = demo_show();
        show.fixture_types.clear();
        let problems = show.validate();
        assert!(problems.iter().any(|p| p.contains("is not in the show")));

        let mut without_modes = demo_fixture_type();
        without_modes.modes.clear();
        show.fixture_types.push(without_modes);
        let problems = show.validate();
        assert!(
            problems.iter().any(|p| p.contains("no DMX modes")),
            "{problems:?}"
        );

        show.fixture_types = vec![demo_fixture_type()];
        show.fixtures[0].mode = "99ch".into();
        let problems = show.validate();
        assert!(problems.iter().any(|p| p.contains("mode `99ch`")));
    }

    #[test]
    fn version_one_shows_load_and_are_upgraded() {
        let path = temp_path("v1");
        fs::write(
            &path,
            r#"{"format":"openlight-show","version":1,"name":"Old","fixture_types":[],"universes":[{"number":1}],"fixtures":[]}"#,
        )
        .unwrap();
        let show = Show::load(&path).unwrap();
        assert_eq!(show.version, SHOW_VERSION);
        assert!(show.palettes.is_empty());
        assert!(show.sequences.is_empty());
        assert!(show.executors.is_empty());
        assert!(show.groups.is_empty());
        assert!(show.worlds.is_empty());
        assert!(show.filters.is_empty());
        assert!(show.effects.is_empty());
        assert!(show.timecodes.is_empty());
        assert!(show.validate().is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn version_two_shows_load_and_are_upgraded() {
        let path = temp_path("v2");
        let mut value = serde_json::to_value(demo_show()).unwrap();
        value["version"] = 2.into();
        {
            let object = value.as_object_mut().unwrap();
            for key in ["groups", "worlds", "filters", "effects", "timecodes"] {
                object.remove(key);
            }
        }
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

        let show = Show::load(&path).unwrap();
        assert_eq!(show.version, SHOW_VERSION);
        assert!(show.groups.is_empty());
        assert!(show.effects.is_empty());
        assert!(show.timecodes.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn version_three_shows_load_and_are_upgraded() {
        let path = temp_path("v3");
        let mut value = serde_json::to_value(demo_show()).unwrap();
        value["version"] = 3.into();
        value.as_object_mut().unwrap().remove("timecodes");
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

        let show = Show::load(&path).unwrap();
        assert_eq!(show.version, SHOW_VERSION);
        assert!(show.timecodes.is_empty());
        assert!(show.validate().is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn version_four_shows_hand_their_programmer_and_masters_to_the_desk() {
        let path = temp_path("v4-upgrade");
        let mut value = serde_json::to_value(demo_show()).unwrap();
        value["version"] = 4.into();
        value["grand_master"] = 0.75.into();
        value["rate_master"] = 2.0.into();
        value["fixtures"][0]["levels"] = serde_json::json!({"Dimmer": 0.5});
        value["groups"] = serde_json::json!([{
            "number": 1, "name": "Front", "fixtures": [1], "master": 0.25
        }]);
        value["executors"] = serde_json::json!([{
            "number": 1, "sequence": 1, "name": "Main", "fader": 0.4
        }]);
        value["sequences"] = serde_json::json!([{"number": 1, "name": "Main", "cues": []}]);
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

        let (show, report) = Show::load_with_report(&path).unwrap();
        assert_eq!(show.version, SHOW_VERSION);
        assert_eq!(report.desk_seed.programmer.get(1, "Dimmer"), Some(0.5));
        assert_eq!(report.desk_seed.grand, Some(0.75));
        assert_eq!(report.desk_seed.rate, Some(2.0));
        assert_eq!(report.desk_seed.groups.get(&1), Some(&0.25));
        assert_eq!(report.desk_seed.executors.get(&1), Some(&0.4));
        assert!(
            report.warnings.iter().any(|warning| warning.contains("v4")),
            "{:?}",
            report.warnings
        );
        assert!(show.validate().is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn timecode_events_are_validated() {
        use crate::timecode::{TimecodeEvent, TimecodeShow};

        let mut show = demo_show();
        show.timecodes.push(TimecodeShow {
            number: 0,
            name: String::new(),
            fps: 0,
            events: vec![
                TimecodeEvent {
                    at: "00:00:02:00".into(),
                    sequence: 9,
                    cue: 1.0,
                },
                TimecodeEvent {
                    at: "00:00:02:00".into(),
                    sequence: 9,
                    cue: 1.0,
                },
                TimecodeEvent {
                    at: "00:00:01:00".into(),
                    sequence: 9,
                    cue: 1.0,
                },
                TimecodeEvent {
                    at: "nope".into(),
                    sequence: 9,
                    cue: 1.0,
                },
            ],
        });
        let problems = show.validate();
        for expected in [
            "timecode number 0",
            "0 fps",
            "two events at 00:00:02:00",
            "not in ascending order",
            "is not a timecode",
            "sequence 9 which is not in the show",
        ] {
            assert!(
                problems.iter().any(|problem| problem.contains(expected)),
                "missing `{expected}` in {problems:#?}"
            );
        }
    }

    #[test]
    fn timecode_events_must_name_a_stored_cue() {
        use crate::playback::{Cue, Sequence, Trigger};
        use crate::timecode::{TimecodeEvent, TimecodeShow};

        let mut show = demo_show();
        show.sequences.push(Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![Cue {
                number: 1.0,
                name: String::new(),
                fade: 0.0,
                delay: 0.0,
                mib: false,
                trigger: Trigger::Go,
                trigger_time: 0.0,
                timing: BTreeMap::new(),
                stagger: 0.0,
                blocked: false,
                shield: BTreeMap::new(),
                levels: Levels::new(),
            }],
        });
        show.timecodes.push(TimecodeShow {
            number: 1,
            name: "Song".into(),
            fps: 25,
            events: vec![TimecodeEvent {
                at: "00:00:00:00".into(),
                sequence: 1,
                cue: 2.0,
            }],
        });
        let problems = show.validate();
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("cue 2 which sequence 1 does not have")),
            "{problems:#?}"
        );
    }

    #[test]
    fn version_five_shows_upgrade_with_go_triggers() {
        use crate::playback::{ExecutorMode, Trigger};

        let mut path = std::env::temp_dir();
        path.push(format!("openlight-{}-v5.olshow", std::process::id()));
        fs::write(
            &path,
            r#"{
                "format": "openlight-show",
                "version": 5,
                "name": "Old",
                "sequences": [{"number": 1, "cues": [{"number": 1}]}],
                "executors": [{"number": 1, "sequence": 1}]
            }"#,
        )
        .unwrap();
        let (show, report) = Show::load_with_report(&path).unwrap();
        assert!(report.warnings.is_empty(), "{report:#?}");
        assert_eq!(show.version, SHOW_VERSION);
        assert_eq!(show.sequences[0].cues[0].trigger, Trigger::Go);
        assert_eq!(show.sequences[0].cues[0].trigger_time, 0.0);
        assert_eq!(show.executors[0].mode, ExecutorMode::Go);

        // The upgraded file validates and writes as the current version.
        show.save(&path).unwrap();
        let (reloaded, _) = Show::load_with_report(&path).unwrap();
        assert_eq!(reloaded, show);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(crate::journal::path_for(&path, ".bak"));
    }

    #[test]
    fn version_six_shows_upgrade_with_an_empty_timing_map() {
        use crate::playback::Trigger;

        let mut path = std::env::temp_dir();
        path.push(format!("openlight-{}-v6.olshow", std::process::id()));
        fs::write(
            &path,
            r#"{
                "format": "openlight-show",
                "version": 6,
                "name": "Old",
                "sequences": [{"number": 1, "cues": [{"number": 1}]}]
            }"#,
        )
        .unwrap();
        let (show, report) = Show::load_with_report(&path).unwrap();
        assert!(report.warnings.is_empty(), "{report:#?}");
        assert_eq!(show.version, SHOW_VERSION);
        let cue = &show.sequences[0].cues[0];
        assert!(cue.timing.is_empty());
        assert_eq!(cue.stagger, 0.0);
        assert_eq!(cue.trigger, Trigger::Go);

        show.save(&path).unwrap();
        let (reloaded, _) = Show::load_with_report(&path).unwrap();
        assert_eq!(reloaded, show);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(crate::journal::path_for(&path, ".bak"));
    }

    #[test]
    fn version_seven_shows_return_an_empty_home_and_report_profile_defaults() {
        let mut show = demo_show();
        show.fixture_types[0].modes[0].channels[0].default_raw = 255;
        let mut value = serde_json::to_value(&show).unwrap();
        value["version"] = serde_json::json!(7);
        let path = temp_path("v7-home");
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

        let (loaded, report) = Show::load_with_report(&path).unwrap();
        assert!(loaded.fixtures[0].home.is_empty());
        assert_eq!(loaded.home_level(&loaded.fixtures[0], "Dimmer"), 1.0);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("Home seeded from profiles")
                    && warning.contains("fixture 1: Dimmer 100%")),
            "{report:#?}"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn version_eight_shows_upgrade_with_no_blocks_or_shields() {
        let path = temp_path("v8-blocked");
        fs::write(
            &path,
            r#"{
                "format": "openlight-show",
                "version": 8,
                "name": "Old",
                "sequences": [{"number": 1, "cues": [{"number": 1}]}]
            }"#,
        )
        .unwrap();
        let (show, _) = Show::load_with_report(&path).unwrap();
        assert_eq!(show.version, SHOW_VERSION);
        let cue = &show.sequences[0].cues[0];
        assert!(!cue.blocked);
        assert!(cue.shield.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn block_and_shield_roundtrip() {
        use crate::playback::{Cue, Sequence, Trigger};

        let mut show = demo_show();
        let mut levels = Levels::new();
        levels.set(1, "Dimmer", 1.0);
        show.sequences.push(Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![Cue {
                number: 1.0,
                name: String::new(),
                fade: 0.0,
                delay: 0.0,
                mib: false,
                trigger: Trigger::Go,
                trigger_time: 0.0,
                timing: BTreeMap::new(),
                stagger: 0.0,
                blocked: true,
                shield: BTreeMap::from([("Dimmer".to_string(), 0.5)]),
                levels,
            }],
        });
        let path = temp_path("blocked-roundtrip");
        show.save(&path).unwrap();
        let loaded = Show::load(&path).unwrap();
        assert!(loaded.sequences[0].cues[0].blocked);
        assert_eq!(loaded.sequences[0].cues[0].shield.get("Dimmer"), Some(&0.5));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_shield_must_name_an_attribute_the_cue_has() {
        use crate::playback::{Cue, Sequence, Trigger};

        let mut show = demo_show();
        let mut levels = Levels::new();
        levels.set(1, "Dimmer", 0.5);
        show.sequences.push(Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![Cue {
                number: 1.0,
                name: String::new(),
                fade: 0.0,
                delay: 0.0,
                mib: false,
                trigger: Trigger::Go,
                trigger_time: 0.0,
                timing: BTreeMap::new(),
                stagger: 0.0,
                blocked: false,
                shield: BTreeMap::from([("Bogus".to_string(), 0.5), ("Dimmer".to_string(), 1.5)]),
                levels,
            }],
        });
        let problems = show.validate();
        for expected in [
            "shield `Bogus` is not an attribute",
            "shield `Dimmer` is 1.5",
        ] {
            assert!(
                problems.iter().any(|problem| problem.contains(expected)),
                "missing `{expected}` in {problems:#?}"
            );
        }
    }

    #[test]
    fn home_resolves_stored_then_profile_then_zero() {
        let mut show = demo_show();
        assert_eq!(show.home_level(&show.fixtures[0], "Dimmer"), 0.0);
        assert!(!show.has_home(&show.fixtures[0], "Pan"));

        show.fixture_types[0].modes[0].channels[1].default_raw = 128;
        let red = show.home_level(&show.fixtures[0], "ColorAdd_R");
        assert!((red - 128.0 / 255.0).abs() < 1e-9);
        assert!(show.has_home(&show.fixtures[0], "ColorAdd_R"));

        show.fixtures[0].home.insert("ColorAdd_R".into(), 0.25);
        assert_eq!(show.home_level(&show.fixtures[0], "ColorAdd_R"), 0.25);

        let path = temp_path("home-roundtrip");
        show.save(&path).unwrap();
        let loaded = Show::load(&path).unwrap();
        assert_eq!(loaded.fixtures[0].home.get("ColorAdd_R"), Some(&0.25));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(crate::journal::path_for(&path, ".bak"));
    }

    #[test]
    fn invalid_home_levels_are_reported() {
        let mut show = demo_show();
        show.fixtures[0].home.insert("Dimmer".into(), 1.5);
        show.fixtures[0].home.insert(" ".into(), 0.5);
        let problems = show.validate();
        for expected in ["home `Dimmer` is 1.5", "home value without an attribute"] {
            assert!(
                problems.iter().any(|problem| problem.contains(expected)),
                "missing `{expected}` in {problems:#?}"
            );
        }
    }

    #[test]
    fn invalid_cue_timing_is_reported() {
        use crate::playback::{AttributeTiming, Cue, Curve, Sequence, Trigger};

        let mut show = demo_show();
        let mut levels = Levels::new();
        levels.set(1, "Dimmer", 0.5);
        let mut timing = BTreeMap::new();
        timing.insert(
            "Bogus".to_string(),
            AttributeTiming {
                fade: Some(-1.0),
                delay: Some(f64::NAN),
                curve: Curve::EaseOut,
            },
        );
        show.sequences.push(Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![Cue {
                number: 1.0,
                name: String::new(),
                fade: 0.0,
                delay: 0.0,
                mib: false,
                trigger: Trigger::Go,
                trigger_time: 0.0,
                timing,
                stagger: -0.5,
                blocked: false,
                shield: BTreeMap::new(),
                levels,
            }],
        });

        let problems = show.validate();
        for expected in [
            "timing fade -1",
            "timing delay NaN",
            "`Bogus` is not an attribute",
            "stagger -0.5",
        ] {
            assert!(
                problems.iter().any(|problem| problem.contains(expected)),
                "missing `{expected}` in {problems:#?}"
            );
        }
    }

    #[test]
    fn invalid_phase_three_objects_are_reported() {
        let mut show = demo_show();
        show.groups.push(Group {
            number: 0,
            name: String::new(),
            fixtures: vec![1, 1],
        });
        show.worlds.push(World {
            number: 0,
            name: String::new(),
            fixtures: Vec::new(),
        });
        show.filters.push(Filter {
            number: 0,
            name: String::new(),
            features: Vec::new(),
        });
        show.effects.push(crate::effect::Effect {
            number: 1,
            name: String::new(),
            attribute: String::new(),
            waveform: crate::effect::Waveform::Sine,
            rate_hz: 1000.0,
            size: 2.0,
            spread: 400.0,
        });

        let problems = show.validate();
        for expected in [
            "group number 0",
            "fixture 1 is listed twice",
            "world number 0",
            "world 0: no fixtures",
            "filter number 0",
            "filter 0: no features",
            "effect 1: no attribute",
            "rate 1000 Hz",
            "size 2 is outside",
            "spread 400",
        ] {
            assert!(
                problems.iter().any(|problem| problem.contains(expected)),
                "missing `{expected}` in {problems:#?}"
            );
        }
    }

    #[test]
    fn invalid_palettes_sequences_and_executors_are_reported() {
        use crate::palette::Palette;
        use crate::playback::{Cue, Executor, ExecutorMode, Sequence, Trigger};

        let mut show = demo_show();
        show.palettes.push(Palette {
            feature: String::new(),
            number: 0,
            name: String::new(),
            values: BTreeMap::new(),
        });
        show.sequences.push(Sequence {
            number: 1,
            name: "Main".into(),
            cues: vec![
                Cue {
                    number: 2.0,
                    name: String::new(),
                    fade: -1.0,
                    delay: 0.0,
                    mib: false,
                    trigger: Trigger::Go,
                    trigger_time: 0.0,
                    timing: BTreeMap::new(),
                    stagger: 0.0,
                    blocked: false,
                    shield: BTreeMap::new(),
                    levels: Levels::new(),
                },
                Cue {
                    number: 1.0,
                    name: String::new(),
                    fade: 0.0,
                    delay: 0.0,
                    mib: false,
                    trigger: Trigger::Go,
                    trigger_time: 0.0,
                    timing: BTreeMap::new(),
                    stagger: 0.0,
                    blocked: false,
                    shield: BTreeMap::new(),
                    levels: Levels::new(),
                },
                Cue {
                    number: 1.0,
                    name: String::new(),
                    fade: 0.0,
                    delay: 0.0,
                    mib: false,
                    trigger: Trigger::Go,
                    trigger_time: 0.0,
                    timing: BTreeMap::new(),
                    stagger: 0.0,
                    blocked: false,
                    shield: BTreeMap::new(),
                    levels: Levels::new(),
                },
            ],
        });
        show.executors.push(Executor {
            number: 1,
            sequence: 9,
            name: String::new(),
            mode: ExecutorMode::Go,
        });

        let problems = show.validate();
        assert!(problems.iter().any(|p| p.contains("feature pool is empty")));
        assert!(problems.iter().any(|p| p.contains("number 0")));
        assert!(problems.iter().any(|p| p.contains("fade -1")));
        assert!(problems.iter().any(|p| p.contains("ascending order")));
        assert!(problems.iter().any(|p| p.contains("defined twice")));
        assert!(problems.iter().any(|p| p.contains("sequence 9")));
    }

    #[test]
    fn save_refuses_invalid_show() {
        let mut show = demo_show();
        show.fixtures[0].address = 600;
        let path = temp_path("invalid");
        assert!(show.save(&path).is_err());
        assert!(!path.exists(), "nothing is written for an invalid show");
    }
}

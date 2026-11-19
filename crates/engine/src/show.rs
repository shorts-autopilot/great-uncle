//! The show file.
//!
//! A show is self-contained: fixture types are embedded, so a show opened on
//! another machine never depends on a local fixture library. The format is
//! versioned and every document is validated on load and before save.
//!
//! Version 2 adds palettes, sequences and executors. Version 1 files still
//! load: the new sections default to empty and the show is upgraded on read.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fixture_type::{FixtureType, Mode};
use crate::palette::Palette;
use crate::patch::{self, PatchRange};
use crate::playback::{Executor, Sequence, MAX_TIME};

pub const SHOW_FORMAT: &str = "openlight-show";
pub const SHOW_VERSION: u32 = 2;
/// Older documents that still load, oldest first.
pub const READABLE_VERSIONS: &[u32] = &[1, 2];
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
    /// Normalised physical level per attribute (0..=1).
    #[serde(default)]
    pub levels: BTreeMap<String, f64>,
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

    pub fn find_type(&self, key: &str) -> Option<&FixtureType> {
        self.fixture_types
            .iter()
            .find(|fixture_type| fixture_type.key().eq_ignore_ascii_case(key.trim()))
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

            for (attribute, level) in &fixture.levels {
                if !level.is_finite() || !(0.0..=1.0).contains(level) {
                    problems.push(format!(
                        "fixture {}: level for `{attribute}` is {level}, expected 0..=1",
                        fixture.number
                    ));
                }
                if mode.channel_for(attribute).is_none() {
                    problems.push(format!(
                        "fixture {}: attribute `{attribute}` is not in mode `{}`",
                        fixture.number, mode.name
                    ));
                }
            }
        }

        for (first, second) in patch::conflicts(&ranges) {
            problems.push(format!(
                "fixtures {first} and {second}: overlapping DMX addresses"
            ));
        }

        self.validate_palettes(&mut problems);
        self.validate_sequences(&mut problems);
        self.validate_executors(&mut problems);

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
            let mut cue_numbers: Vec<f64> = Vec::new();
            for cue in &sequence.cues {
                if !cue.number.is_finite() || cue.number < 0.0 {
                    problems.push(format!(
                        "sequence {}: cue number {} is not a finite positive number",
                        sequence.number, cue.number
                    ));
                }
                if cue_numbers
                    .iter()
                    .any(|number| (*number - cue.number).abs() < 1e-9)
                {
                    problems.push(format!(
                        "sequence {}: cue {} is defined twice",
                        sequence.number,
                        crate::playback::format_cue_number(cue.number)
                    ));
                }
                cue_numbers.push(cue.number);
                for (name, value) in [("fade", cue.fade), ("delay", cue.delay)] {
                    if !value.is_finite() || !(0.0..=MAX_TIME).contains(&value) {
                        problems.push(format!(
                            "sequence {} cue {}: {name} {value} is outside 0..={MAX_TIME} seconds",
                            sequence.number,
                            crate::playback::format_cue_number(cue.number)
                        ));
                    }
                }
                for (fixture, attributes) in cue.levels.fixtures() {
                    for (attribute, level) in attributes {
                        if attribute.trim().is_empty() {
                            problems.push(format!(
                                "sequence {} cue {}: value without an attribute",
                                sequence.number,
                                crate::playback::format_cue_number(cue.number)
                            ));
                        }
                        if !level.is_finite() || !(0.0..=1.0).contains(level) {
                            problems.push(format!(
                                "sequence {} cue {}: fixture {fixture} `{attribute}` is {level}, expected 0..=1",
                                sequence.number,
                                crate::playback::format_cue_number(cue.number)
                            ));
                        }
                    }
                }
            }
            for pair in sequence.cues.windows(2) {
                if pair[0].number > pair[1].number {
                    problems.push(format!(
                        "sequence {}: cue {} comes after cue {} and is not in ascending order",
                        sequence.number,
                        crate::playback::format_cue_number(pair[1].number),
                        crate::playback::format_cue_number(pair[0].number)
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
            if !executor.fader.is_finite() || !(0.0..=1.0).contains(&executor.fader) {
                problems.push(format!(
                    "executor {}: fader {} is outside 0..=1",
                    executor.number, executor.fader
                ));
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

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| ShowError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }

        let mut temp_name: OsString = path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| OsString::from("show"));
        temp_name.push(".tmp");
        let temp = path.with_file_name(temp_name);

        {
            let mut file = fs::File::create(&temp).map_err(|source| ShowError::Io {
                path: temp.clone(),
                source,
            })?;
            file.write_all(json.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|source| ShowError::Io {
                    path: temp.clone(),
                    source,
                })?;
        }
        fs::rename(&temp, path).map_err(|source| ShowError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Read and validate a show. A failed load never leaves a half-built show.
    pub fn load(path: &Path) -> Result<Show, ShowError> {
        let text = fs::read_to_string(path).map_err(|source| ShowError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let value: serde_json::Value =
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

        let mut show: Show = serde_json::from_value(value).map_err(|source| ShowError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        // Version 1 has no palettes, sequences or executors. They default to
        // empty, so the document is upgraded in place on read and saved as v2.
        show.version = SHOW_VERSION;
        let problems = show.validate();
        if !problems.is_empty() {
            return Err(ShowError::Invalid {
                path: path.to_path_buf(),
                problems,
            });
        }
        Ok(show)
    }
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
        assert!(text.contains("\"version\": 2"));
        assert!(text.contains("OpenLight Demo Wash"), "types are embedded");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_rejects_future_versions() {
        let path = temp_path("future");
        fs::write(
            &path,
            r#"{"format":"openlight-show","version":9,"name":"x","fixture_types":[],"universes":[],"fixtures":[]}"#,
        )
        .unwrap();
        let error = Show::load(&path).unwrap_err();
        assert!(matches!(
            error,
            ShowError::UnsupportedVersion { found: 9, .. }
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
            levels: BTreeMap::new(),
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
    fn out_of_range_level_is_reported() {
        let mut show = demo_show();
        show.fixtures[0].levels.insert("Dimmer".into(), 2.5);
        let problems = show.validate();
        assert!(problems.iter().any(|p| p.contains("expected 0..=1")));
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
        assert!(show.validate().is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn invalid_palettes_sequences_and_executors_are_reported() {
        use crate::levels::Levels;
        use crate::palette::Palette;
        use crate::playback::{Cue, Executor, Sequence};

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
                    levels: Levels::new(),
                },
                Cue {
                    number: 1.0,
                    name: String::new(),
                    fade: 0.0,
                    delay: 0.0,
                    levels: Levels::new(),
                },
            ],
        });
        show.executors.push(Executor {
            number: 1,
            sequence: 9,
            name: String::new(),
            fader: 2.0,
        });

        let problems = show.validate();
        assert!(problems.iter().any(|p| p.contains("feature pool is empty")));
        assert!(problems.iter().any(|p| p.contains("number 0")));
        assert!(problems.iter().any(|p| p.contains("fade -1")));
        assert!(problems.iter().any(|p| p.contains("ascending order")));
        assert!(problems.iter().any(|p| p.contains("sequence 9")));
        assert!(problems.iter().any(|p| p.contains("fader 2")));
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

//! The show file.
//!
//! A show is self-contained: fixture types are embedded, so a show opened on
//! another machine never depends on a local fixture library. The format is
//! versioned and every document is validated on load and before save.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fixture_type::{FixtureType, Mode};
use crate::patch::{self, PatchRange};

pub const SHOW_FORMAT: &str = "openlight-show";
pub const SHOW_VERSION: u32 = 1;
pub const MAX_UNIVERSE_SIZE: u16 = 512;
pub const MAX_UNIVERSE_NUMBER: u16 = 63_999;
pub const MAX_FIXTURES: usize = 4096;

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

        problems
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
        if version != SHOW_VERSION {
            return Err(ShowError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: version,
                supported: SHOW_VERSION,
            });
        }

        let show: Show = serde_json::from_value(value).map_err(|source| ShowError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
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
        "{}: show version {found} is not supported (this build reads {supported})",
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
        assert!(text.contains("\"version\": 1"));
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
    fn save_refuses_invalid_show() {
        let mut show = demo_show();
        show.fixtures[0].address = 600;
        let path = temp_path("invalid");
        assert!(show.save(&path).is_err());
        assert!(!path.exists(), "nothing is written for an invalid show");
    }
}

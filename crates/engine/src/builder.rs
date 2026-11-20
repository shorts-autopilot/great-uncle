//! Portable fixture-type files (`.olft`) and their validation.
//!
//! A profile is data, so this module is the one door for file import/export
//! and command-line edits. The JSON format is deliberately flat and versioned;
//! every builder command and every imported file runs the same checks, so a
//! profile cannot be valid through one surface and invalid through another.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fixture_type::{AttributeDef, Channel, FixtureType, Mode};
use crate::show::MAX_UNIVERSE_SIZE;

pub const FIXTURE_TYPE_FORMAT: &str = "openlight-fixture-type";
pub const FIXTURE_TYPE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureTypeFile {
    pub format: String,
    pub version: u32,
    pub manufacturer: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub attributes: Vec<AttributeDef>,
    #[serde(default)]
    pub modes: Vec<Mode>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub home: BTreeMap<String, f64>,
}

impl FixtureTypeFile {
    pub fn from_type(fixture_type: &FixtureType) -> Self {
        Self {
            format: FIXTURE_TYPE_FORMAT.into(),
            version: FIXTURE_TYPE_VERSION,
            manufacturer: fixture_type.manufacturer.clone(),
            name: fixture_type.name.clone(),
            description: fixture_type.description.clone(),
            attributes: fixture_type.attributes.clone(),
            modes: fixture_type.modes.clone(),
            home: fixture_type.home.clone(),
        }
    }

    pub fn into_type(self) -> FixtureType {
        FixtureType {
            manufacturer: self.manufacturer,
            name: self.name,
            description: self.description,
            attributes: self.attributes,
            modes: self.modes,
            home: self.home,
        }
    }

    pub fn key(&self) -> String {
        format!("{} {}", self.manufacturer.trim(), self.name.trim())
            .trim()
            .to_string()
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
        let file: Self = serde_json::from_str(&text)
            .map_err(|error| format!("`{}` is not a fixture type file: {error}", path.display()))?;
        file.validate()?;
        Ok(file)
    }

    pub fn to_file(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("cannot encode fixture type: {error}"))?;
        fs::write(path, format!("{text}\n"))
            .map_err(|error| format!("cannot write `{}`: {error}", path.display()))
    }

    /// Check the file's shape and every field the renderer relies on. The
    /// message names the field and, for arrays, the index.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != FIXTURE_TYPE_FORMAT {
            return Err(format!(
                "format is `{}`, expected `{FIXTURE_TYPE_FORMAT}`",
                self.format
            ));
        }
        if self.version != FIXTURE_TYPE_VERSION {
            return Err(format!(
                "version is {}, expected {FIXTURE_TYPE_VERSION}",
                self.version
            ));
        }
        if self.key().is_empty() {
            return Err("manufacturer and name are both empty".into());
        }
        if self.modes.is_empty() {
            return Err("modes: at least one mode is required".into());
        }

        let mut attributes = BTreeSet::new();
        for (index, attribute) in self.attributes.iter().enumerate() {
            if attribute.name.trim().is_empty() {
                return Err(format!("attributes[{index}].name is empty"));
            }
            if !attributes.insert(attribute.name.trim().to_ascii_lowercase()) {
                return Err(format!(
                    "attributes[{index}].name `{}` is defined twice",
                    attribute.name
                ));
            }
        }

        let mut mode_names = BTreeSet::new();
        for (mode_index, mode) in self.modes.iter().enumerate() {
            if mode.name.trim().is_empty() {
                return Err(format!("modes[{mode_index}].name is empty"));
            }
            if !mode_names.insert(mode.name.trim().to_ascii_lowercase()) {
                return Err(format!(
                    "modes[{mode_index}].name `{}` is defined twice",
                    mode.name
                ));
            }
            if mode.channels.is_empty() {
                return Err(format!("modes[{mode_index}].channels is empty"));
            }

            let mut slots = BTreeSet::new();
            for (channel_index, channel) in mode.channels.iter().enumerate() {
                let field = format!("modes[{mode_index}].channels[{channel_index}]");
                if channel.offset == 0 || channel.offset > MAX_UNIVERSE_SIZE {
                    return Err(format!(
                        "{field}.offset {} is outside 1..={MAX_UNIVERSE_SIZE}",
                        channel.offset
                    ));
                }
                if !slots.insert(channel.offset) {
                    return Err(format!("{field}.offset {} is used twice", channel.offset));
                }
                if let Some(fine) = channel.fine_offset {
                    if fine == 0 || fine > MAX_UNIVERSE_SIZE {
                        return Err(format!(
                            "{field}.fine_offset {fine} is outside 1..={MAX_UNIVERSE_SIZE}"
                        ));
                    }
                    if fine == channel.offset {
                        return Err(format!(
                            "{field}.fine_offset {fine} is the same as its coarse offset"
                        ));
                    }
                    if !slots.insert(fine) {
                        return Err(format!("{field}.fine_offset {fine} is used twice"));
                    }
                }
                if channel.attribute.trim().is_empty() {
                    return Err(format!("{field}.attribute is empty"));
                }
                if !attributes.contains(&channel.attribute.trim().to_ascii_lowercase()) {
                    return Err(format!(
                        "{field}.attribute `{}` is not declared in attributes",
                        channel.attribute
                    ));
                }
                for (function_index, function) in channel.functions.iter().enumerate() {
                    let function_field = format!("{field}.functions[{function_index}]");
                    if function.dmx_from > function.dmx_to {
                        return Err(format!(
                            "{function_field}: dmx_from {} is greater than dmx_to {}",
                            function.dmx_from, function.dmx_to
                        ));
                    }
                    if !function.physical_from.is_finite() || !function.physical_to.is_finite() {
                        return Err(format!("{function_field}: physical range is not finite"));
                    }
                }
            }
            if mode.footprint() > MAX_UNIVERSE_SIZE {
                return Err(format!(
                    "modes[{mode_index}] footprint {} exceeds {MAX_UNIVERSE_SIZE}",
                    mode.footprint()
                ));
            }
        }

        let mut home_names = BTreeSet::new();
        for (attribute, level) in &self.home {
            if !attributes.contains(&attribute.trim().to_ascii_lowercase()) {
                return Err(format!("home `{attribute}` is not declared in attributes"));
            }
            if !home_names.insert(attribute.trim().to_ascii_lowercase()) {
                return Err(format!("home `{attribute}` is defined twice"));
            }
            if !level.is_finite() || !(0.0..=1.0).contains(level) {
                return Err(format!("home `{attribute}` is {level}, expected 0..=1"));
            }
        }
        Ok(())
    }
}

/// A small factory used by `Type New` and the command-line channel editor.
pub fn new_type(manufacturer: &str, name: &str) -> FixtureType {
    let attribute = AttributeDef {
        name: "Dimmer".into(),
        pretty: "Dimmer".into(),
        feature: "Dimmer".into(),
        unit: "Percent".into(),
    };
    let channel = Channel {
        offset: 1,
        fine_offset: None,
        attribute: "Dimmer".into(),
        default_raw: 0,
        functions: vec![crate::fixture_type::ChannelFunction {
            name: "Dimmer".into(),
            dmx_from: 0,
            dmx_to: 255,
            physical_from: 0.0,
            physical_to: 100.0,
        }],
    };
    FixtureType {
        manufacturer: manufacturer.trim().to_string(),
        name: name.trim().to_string(),
        description: String::new(),
        attributes: vec![attribute],
        modes: vec![Mode {
            name: "Default".into(),
            channels: vec![channel],
        }],
        home: Default::default(),
    }
}

pub fn attribute_for(name: &str, pretty: &str) -> AttributeDef {
    let lower = name.to_ascii_lowercase();
    let feature = if lower.starts_with("coloradd_") || lower == "color" {
        "Color"
    } else if lower == "dimmer" {
        "Dimmer"
    } else {
        ""
    };
    AttributeDef {
        name: name.trim().to_string(),
        pretty: if pretty.trim().is_empty() {
            name.trim().to_string()
        } else {
            pretty.trim().to_string()
        },
        feature: feature.into(),
        unit: String::new(),
    }
}

pub fn channel_for(offset: u16, attribute: &str, name: &str) -> Channel {
    let physical_to = if attribute.eq_ignore_ascii_case("Dimmer") {
        100.0
    } else {
        1.0
    };
    Channel {
        offset,
        fine_offset: None,
        attribute: attribute.trim().to_string(),
        default_raw: 0,
        functions: vec![crate::fixture_type::ChannelFunction {
            name: if name.trim().is_empty() {
                attribute.trim().to_string()
            } else {
                name.trim().to_string()
            },
            dmx_from: 0,
            dmx_to: 255,
            physical_from: 0.0,
            physical_to,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> FixtureTypeFile {
        FixtureTypeFile {
            format: FIXTURE_TYPE_FORMAT.into(),
            version: FIXTURE_TYPE_VERSION,
            manufacturer: "Generic".into(),
            name: "4ch".into(),
            description: String::new(),
            attributes: vec![
                attribute_for("Dimmer", "Dimmer"),
                attribute_for("ColorAdd_R", "Red"),
            ],
            modes: vec![Mode {
                name: "4ch".into(),
                channels: vec![
                    channel_for(1, "Dimmer", "Dimmer"),
                    channel_for(2, "ColorAdd_R", "Red"),
                ],
            }],
            home: BTreeMap::from([("ColorAdd_R".to_string(), 1.0)]),
        }
    }

    fn temporary(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "openlight-builder-{}-{name}-{:?}.olft",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn a_type_round_trips_through_an_olft_file() {
        let file = sample();
        let path = temporary("roundtrip");
        file.to_file(&path).unwrap();
        let loaded = FixtureTypeFile::from_file(&path).unwrap();
        assert_eq!(loaded, file);
        assert_eq!(loaded.into_type().home["ColorAdd_R"], 1.0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn duplicate_offsets_name_the_index() {
        let mut file = sample();
        file.modes[0].channels[1].offset = 1;
        let error = file.validate().unwrap_err();
        assert!(error.contains("channels[1].offset 1"), "{error}");
    }

    #[test]
    fn an_undeclared_attribute_names_the_channel() {
        let mut file = sample();
        file.modes[0].channels[1].attribute = "ColorAdd_B".into();
        let error = file.validate().unwrap_err();
        assert!(error.contains("channels[1].attribute"), "{error}");
        assert!(error.contains("ColorAdd_B"), "{error}");
    }

    #[test]
    fn a_reversed_function_range_is_rejected() {
        let mut file = sample();
        file.modes[0].channels[0].functions[0].dmx_from = 200;
        file.modes[0].channels[0].functions[0].dmx_to = 10;
        let error = file.validate().unwrap_err();
        assert!(error.contains("functions[0]"), "{error}");
        assert!(error.contains("dmx_from"), "{error}");
    }

    #[test]
    fn a_footprint_past_512_is_rejected() {
        let mut file = sample();
        file.modes[0].channels[0].offset = MAX_UNIVERSE_SIZE + 1;
        let error = file.validate().unwrap_err();
        assert!(error.contains("offset"), "{error}");
    }

    #[test]
    fn a_wrong_format_and_duplicate_mode_are_rejected() {
        let mut file = sample();
        file.format = "something-else".into();
        assert!(file.validate().unwrap_err().contains("format"));
        file.format = FIXTURE_TYPE_FORMAT.into();
        file.modes.push(file.modes[0].clone());
        assert!(file.validate().unwrap_err().contains("defined twice"));
    }

    #[test]
    fn duplicate_home_names_are_rejected_case_insensitively() {
        let mut file = sample();
        file.home.insert("coloradd_r".into(), 0.5);
        assert!(file.validate().unwrap_err().contains("defined twice"));
    }
}

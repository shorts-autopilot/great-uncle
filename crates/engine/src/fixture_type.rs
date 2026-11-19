//! Fixture types, DMX modes and the physical-to-DMX mapping.
//!
//! A fixture type is a reduced GDTF device description: attributes, modes, the
//! channels of each mode and the channel functions that map a physical value to
//! a DMX range. Programming is stored as a normalised physical level per
//! attribute and converted to DMX at render time, so a show stays meaningful if
//! a fixture type gains a more accurate curve later.

use serde::{Deserialize, Serialize};

/// Attribute name used by `At`/`Full`/`Off` intensity commands.
pub const DIMMER: &str = "Dimmer";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureType {
    pub manufacturer: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub attributes: Vec<AttributeDef>,
    #[serde(default)]
    pub modes: Vec<Mode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributeDef {
    pub name: String,
    #[serde(default)]
    pub pretty: String,
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    pub name: String,
    #[serde(default)]
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    /// 1-based DMX offset inside the fixture.
    pub offset: u16,
    pub attribute: String,
    #[serde(default)]
    pub default_raw: u8,
    #[serde(default)]
    pub functions: Vec<ChannelFunction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelFunction {
    pub name: String,
    pub dmx_from: u8,
    pub dmx_to: u8,
    pub physical_from: f64,
    pub physical_to: f64,
}

impl FixtureType {
    /// Unique library key: manufacturer and model, e.g. `OpenLight Demo Wash`.
    pub fn key(&self) -> String {
        format!("{} {}", self.manufacturer.trim(), self.name.trim())
            .trim()
            .to_string()
    }

    pub fn mode(&self, name: &str) -> Option<&Mode> {
        self.modes
            .iter()
            .find(|mode| mode.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn default_mode(&self) -> Option<&Mode> {
        self.modes.first()
    }

    pub fn attribute(&self, name: &str) -> Option<&AttributeDef> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name.eq_ignore_ascii_case(name))
    }

    /// Display name for an attribute, falling back to the technical name.
    pub fn pretty_attribute(&self, name: &str) -> String {
        match self.attribute(name) {
            Some(attribute) if !attribute.pretty.is_empty() => attribute.pretty.clone(),
            _ => name.to_string(),
        }
    }
}

impl Mode {
    /// Number of DMX channels the fixture occupies in this mode.
    pub fn footprint(&self) -> u16 {
        self.channels
            .iter()
            .map(|channel| channel.offset)
            .max()
            .unwrap_or(0)
    }

    pub fn channel_for(&self, attribute: &str) -> Option<&Channel> {
        self.channels
            .iter()
            .find(|channel| channel.attribute.eq_ignore_ascii_case(attribute))
    }
}

impl Default for ChannelFunction {
    fn default() -> Self {
        Self {
            name: String::new(),
            dmx_from: 0,
            dmx_to: 255,
            physical_from: 0.0,
            physical_to: 1.0,
        }
    }
}

impl Channel {
    /// Convert a normalised level (0..=1) into the raw DMX byte for this channel.
    ///
    /// The level selects a physical value across the whole attribute range and
    /// the containing channel function interpolates it to DMX. Attributes with
    /// several discontinuous functions (shutters, colour wheels) are not
    /// programmed in Phase 1; the first function covering the value wins.
    pub fn raw_from_level(&self, level: f64) -> u8 {
        let level = if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        };

        if self.functions.is_empty() {
            return (level * 255.0).round() as u8;
        }

        let mut physical_min = f64::INFINITY;
        let mut physical_max = f64::NEG_INFINITY;
        for function in &self.functions {
            physical_min = physical_min.min(function.physical_from.min(function.physical_to));
            physical_max = physical_max.max(function.physical_from.max(function.physical_to));
        }
        let target = if physical_max > physical_min {
            physical_min + level * (physical_max - physical_min)
        } else {
            physical_min
        };

        // A value exactly on the boundary between two bands belongs to the band
        // that starts there, so a 0..50 / 50..100 split maps 50% into the upper
        // one instead of pinning it to the top of the lower one.
        let chosen = self
            .functions
            .iter()
            .find(|function| {
                let low = function.physical_from.min(function.physical_to);
                let high = function.physical_from.max(function.physical_to);
                high > low && (low - target).abs() < 1e-9
            })
            .or_else(|| {
                self.functions.iter().find(|function| {
                    let low = function.physical_from.min(function.physical_to);
                    let high = function.physical_from.max(function.physical_to);
                    target >= low && target <= high
                })
            });
        if let Some(function) = chosen {
            let low = function.physical_from.min(function.physical_to);
            let high = function.physical_from.max(function.physical_to);
            let fraction = if high > low {
                (target - low) / (high - low)
            } else {
                0.0
            };
            return interpolate(function.dmx_from, function.dmx_to, fraction);
        }

        // The value falls into a gap between functions: snap to the nearest end.
        let mut best = (f64::INFINITY, self.functions[0].dmx_from);
        for function in &self.functions {
            for (physical, raw) in [
                (function.physical_from, function.dmx_from),
                (function.physical_to, function.dmx_to),
            ] {
                let distance = (physical - target).abs();
                if distance < best.0 {
                    best = (distance, raw);
                }
            }
        }
        best.1
    }
}

fn interpolate(from: u8, to: u8, fraction: f64) -> u8 {
    let raw = f64::from(from) + fraction.clamp(0.0, 1.0) * (f64::from(to) - f64::from(from));
    raw.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dimmer_channel(functions: Vec<ChannelFunction>) -> Channel {
        Channel {
            offset: 1,
            attribute: DIMMER.into(),
            default_raw: 0,
            functions,
        }
    }

    fn function(from: f64, to: f64, dmx_from: u8, dmx_to: u8) -> ChannelFunction {
        ChannelFunction {
            name: "fn".into(),
            dmx_from,
            dmx_to,
            physical_from: from,
            physical_to: to,
        }
    }

    #[test]
    fn linear_dimmer_maps_percent_to_dmx() {
        let channel = dimmer_channel(vec![function(0.0, 100.0, 0, 255)]);
        assert_eq!(channel.raw_from_level(0.0), 0);
        assert_eq!(channel.raw_from_level(0.5), 128);
        assert_eq!(channel.raw_from_level(1.0), 255);
        assert_eq!(channel.raw_from_level(2.0), 255, "clamped");
        assert_eq!(channel.raw_from_level(-1.0), 0, "clamped");
    }

    #[test]
    fn channel_without_functions_is_linear() {
        let channel = dimmer_channel(vec![]);
        assert_eq!(channel.raw_from_level(0.5), 128);
        assert_eq!(channel.raw_from_level(f64::NAN), 0);
    }

    #[test]
    fn functions_cover_their_own_dmx_band() {
        // 0..50% -> 10..20, 50..100% -> 100..200
        let channel = dimmer_channel(vec![
            function(0.0, 50.0, 10, 20),
            function(50.0, 100.0, 100, 200),
        ]);
        assert_eq!(channel.raw_from_level(0.0), 10);
        assert_eq!(channel.raw_from_level(0.25), 15);
        assert_eq!(channel.raw_from_level(0.5), 100);
        assert_eq!(channel.raw_from_level(0.75), 150);
        assert_eq!(channel.raw_from_level(1.0), 200);
    }

    #[test]
    fn inverted_physical_range_still_maps() {
        // Physical 100..0 descending over DMX 0..255.
        let channel = dimmer_channel(vec![function(100.0, 0.0, 0, 255)]);
        assert_eq!(channel.raw_from_level(0.0), 0);
        assert_eq!(channel.raw_from_level(1.0), 255);
    }

    #[test]
    fn footprint_is_highest_offset() {
        let mode = Mode {
            name: "4ch".into(),
            channels: vec![
                Channel {
                    offset: 1,
                    attribute: "Dimmer".into(),
                    default_raw: 0,
                    functions: vec![],
                },
                Channel {
                    offset: 4,
                    attribute: "ColorAdd_B".into(),
                    default_raw: 0,
                    functions: vec![],
                },
            ],
        };
        assert_eq!(mode.footprint(), 4);
        assert_eq!(
            Mode {
                name: "empty".into(),
                channels: vec![]
            }
            .footprint(),
            0
        );
    }
}

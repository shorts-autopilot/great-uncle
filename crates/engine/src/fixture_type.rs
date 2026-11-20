//! Fixture types, DMX modes and the physical-to-DMX mapping.
//!
//! A fixture type is a reduced GDTF device description: attributes, modes, the
//! channels of each mode and the channel functions that map a physical value to
//! a DMX range. A channel may carry a fine DMX slot for 16-bit precision, and
//! its functions double as named sets (`Gobo 2`, `Rotate CW`) that commands can
//! address. Programming is stored as a normalised physical level per attribute
//! and converted to DMX at render time, so a show stays meaningful if a fixture
//! type gains a more accurate curve later.

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
    /// 1-based DMX offset inside the fixture: the coarse byte.
    pub offset: u16,
    /// The low byte of a 16-bit attribute, 1-based. Missing means 8-bit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_offset: Option<u16>,
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
    /// Number of DMX channels the fixture occupies in this mode. A 16-bit
    /// channel's fine slot counts even when it sits past every coarse slot.
    pub fn footprint(&self) -> u16 {
        self.channels
            .iter()
            .map(|channel| channel.offset.max(channel.fine_offset.unwrap_or(0)))
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

/// Which channel set a command names: a 1-based index in DMX order, or an
/// exact, case-insensitive name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetSelector<'a> {
    Index(usize),
    Name(&'a str),
}

impl Channel {
    /// The attribute's physical range across every function.
    pub fn physical_range(&self) -> Option<(f64, f64)> {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for function in &self.functions {
            min = min.min(function.physical_from.min(function.physical_to));
            max = max.max(function.physical_from.max(function.physical_to));
        }
        (min.is_finite() && max.is_finite()).then_some((min, max))
    }

    /// Convert a raw DMX byte back into a normalised level (0..=1).
    ///
    /// The 8-bit form of [`Channel::level_from_raw16`]: the coarse byte is
    /// expanded by 257, so an 8-bit channel keeps the fractions it always had.
    pub fn level_from_raw(&self, raw: u8) -> f64 {
        self.level_from_raw16(u16::from(raw) * 257)
    }

    /// Convert a 16-bit DMX value back into a normalised level.
    ///
    /// The inverse of [`Channel::raw16_from_level`]: the function whose DMX
    /// band contains `raw` is chosen, its physical value is interpolated, and
    /// that is normalised across the attribute's whole physical range. A value
    /// in a gap between bands snaps to the nearest band end.
    pub fn level_from_raw16(&self, raw: u16) -> f64 {
        if self.functions.is_empty() {
            return f64::from(raw) / 65535.0;
        }
        let Some((physical_min, physical_max)) = self.physical_range() else {
            return 0.0;
        };
        // The band is named by the coarse byte, so every low byte under that
        // byte stays inside the same set.
        let coarse = (raw >> 8) as u8;

        let physical = match self.functions.iter().find(|function| {
            let low = function.dmx_from.min(function.dmx_to);
            let high = function.dmx_from.max(function.dmx_to);
            coarse >= low && coarse <= high
        }) {
            Some(function) => {
                let low = f64::from(function.dmx_from.min(function.dmx_to)) * 257.0;
                let high = f64::from(function.dmx_from.max(function.dmx_to)) * 257.0;
                let span = high - low;
                let fraction = if span == 0.0 {
                    0.0
                } else {
                    (f64::from(raw) - low) / span
                };
                function.physical_from
                    + (function.physical_to - function.physical_from) * fraction.clamp(0.0, 1.0)
            }
            None => {
                let mut best = (f64::INFINITY, physical_min);
                for function in &self.functions {
                    for (end, physical) in [
                        (function.dmx_from, function.physical_from),
                        (function.dmx_to, function.physical_to),
                    ] {
                        let distance = (f64::from(raw) - f64::from(end) * 257.0).abs();
                        if distance < best.0 {
                            best = (distance, physical);
                        }
                    }
                }
                best.1
            }
        };

        if physical_max > physical_min {
            ((physical - physical_min) / (physical_max - physical_min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Convert a normalised level into this channel's 16-bit DMX value.
    ///
    /// The level selects a physical value across the whole attribute range and
    /// the containing function interpolates it to DMX. An 8-bit profile keeps
    /// its byte rounding and expands the byte by 257; a channel with a fine
    /// offset uses the whole 0..=65535 grid, so 50% splits `0x7F 0xFF`.
    pub fn raw16_from_level(&self, level: f64) -> u16 {
        let level = if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let sixteen_bit = self.fine_offset.is_some();

        if self.functions.is_empty() {
            return if sixteen_bit {
                (level * 65535.0).floor() as u16
            } else {
                (level * 255.0).round() as u16 * 257
            };
        }
        let Some((physical_min, physical_max)) = self.physical_range() else {
            return 0;
        };
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
            return if sixteen_bit {
                interpolate16(function.dmx_from, function.dmx_to, fraction)
            } else {
                u16::from(interpolate(function.dmx_from, function.dmx_to, fraction)) * 257
            };
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
        if sixteen_bit {
            interpolate16(best.1, best.1, 0.0)
        } else {
            u16::from(best.1) * 257
        }
    }

    /// The raw coarse byte for this channel.
    pub fn raw_from_level(&self, level: f64) -> u8 {
        (self.raw16_from_level(level) >> 8) as u8
    }

    /// Every set in DMX order, with its 1-based index.
    pub fn sets(&self) -> impl Iterator<Item = (usize, &ChannelFunction)> {
        self.functions
            .iter()
            .enumerate()
            .map(|(index, function)| (index + 1, function))
    }

    /// The set a selector names, if it exists.
    pub fn set_for(&self, selector: SetSelector<'_>) -> Option<(usize, &ChannelFunction)> {
        match selector {
            SetSelector::Index(index) => self.sets().find(|(number, _)| *number == index),
            SetSelector::Name(name) => self
                .sets()
                .find(|(_, function)| function.name.eq_ignore_ascii_case(name.trim())),
        }
    }

    /// The normalised midpoint of a set's physical range.
    ///
    /// Storing the midpoint makes a set an ordinary level, so it fades, stores
    /// and prints like any other value; the listing shows the DMX byte it
    /// actually lands on.
    pub fn level_for_set(&self, selector: SetSelector<'_>) -> Option<f64> {
        let (_, function) = self.set_for(selector)?;
        let (min, max) = self.physical_range()?;
        let midpoint = (function.physical_from + function.physical_to) / 2.0;
        Some(if max > min {
            ((midpoint - min) / (max - min)).clamp(0.0, 1.0)
        } else {
            0.0
        })
    }
}

fn interpolate(from: u8, to: u8, fraction: f64) -> u8 {
    let raw = f64::from(from) + fraction.clamp(0.0, 1.0) * (f64::from(to) - f64::from(from));
    raw.round().clamp(0.0, 255.0) as u8
}

/// 16-bit interpolation across a band's coarse endpoints, mapped through 257.
fn interpolate16(from: u8, to: u8, fraction: f64) -> u16 {
    let low = f64::from(from) * 257.0;
    let high = f64::from(to) * 257.0;
    let raw = low + fraction.clamp(0.0, 1.0) * (high - low);
    raw.floor().clamp(0.0, 65535.0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dimmer_channel(functions: Vec<ChannelFunction>) -> Channel {
        Channel {
            offset: 1,
            fine_offset: None,
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

    fn named_function(name: &str, from: f64, to: f64, dmx_from: u8, dmx_to: u8) -> ChannelFunction {
        ChannelFunction {
            name: name.into(),
            ..function(from, to, dmx_from, dmx_to)
        }
    }

    fn pan_channel() -> Channel {
        Channel {
            fine_offset: Some(2),
            attribute: "Pan".into(),
            ..dimmer_channel(Vec::new())
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
    fn level_from_raw_inverts_a_linear_channel() {
        let channel = dimmer_channel(vec![function(0.0, 100.0, 0, 255)]);
        assert_eq!(channel.level_from_raw(0), 0.0);
        assert!((channel.level_from_raw(128) - 128.0 / 255.0).abs() < 1e-9);
        assert_eq!(channel.level_from_raw(255), 1.0);

        for raw in [0u8, 1, 77, 128, 200, 255] {
            let level = channel.level_from_raw(raw);
            assert_eq!(channel.raw_from_level(level), raw, "raw {raw} round-trips");
        }
    }

    #[test]
    fn level_from_raw_picks_the_band_that_contains_the_byte() {
        let channel = dimmer_channel(vec![
            function(0.0, 50.0, 10, 20),
            function(50.0, 100.0, 100, 200),
        ]);
        assert!((channel.level_from_raw(15) - 0.25).abs() < 1e-9);
        assert!((channel.level_from_raw(150) - 0.75).abs() < 1e-9);
        // 20 is shared with band 2's first byte; one byte cannot name both
        // ends, and the lower band wins by construction.
        for raw in [10u8, 15, 100, 150, 200] {
            let level = channel.level_from_raw(raw);
            assert_eq!(channel.raw_from_level(level), raw, "raw {raw}");
        }
    }

    #[test]
    fn level_from_raw_snaps_from_a_gap_to_the_nearest_end() {
        let channel = dimmer_channel(vec![
            function(0.0, 50.0, 10, 20),
            function(50.0, 100.0, 100, 200),
        ]);
        assert_eq!(channel.level_from_raw(0), 0.0, "below every band");
        assert!((channel.level_from_raw(90) - 0.5).abs() < 1e-9, "near 50%");
        assert_eq!(channel.level_from_raw(255), 1.0, "above every band");
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
    fn sixteen_bit_levels_split_into_two_bytes() {
        let channel = pan_channel();
        assert_eq!(channel.raw16_from_level(0.0), 0);
        assert_eq!(channel.raw16_from_level(0.5), 32767);
        assert_eq!(channel.raw16_from_level(1.0), 65535);
        assert_eq!(
            (
                (channel.raw16_from_level(0.5) >> 8) as u8,
                channel.raw16_from_level(0.5) as u8
            ),
            (127, 255),
            "0x7F 0xFF is the midpoint"
        );
        for level in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            assert_eq!(
                channel.raw_from_level(level),
                (channel.raw16_from_level(level) >> 8) as u8,
                "the coarse byte is the high byte at {level}"
            );
        }
    }

    #[test]
    fn sixteen_bit_bands_pick_the_same_band_as_eight_bit() {
        let functions = vec![function(0.0, 50.0, 0, 15), function(50.0, 100.0, 16, 255)];
        let eight = dimmer_channel(functions.clone());
        let sixteen = Channel {
            fine_offset: Some(2),
            functions,
            ..eight.clone()
        };
        for level in [0.0, 0.25, 0.5, 0.6, 1.0] {
            let wide = (sixteen.raw16_from_level(level) >> 8) as u8;
            let narrow = eight.raw_from_level(level);
            let band = eight
                .functions
                .iter()
                .find(|function| narrow >= function.dmx_from && narrow <= function.dmx_to)
                .expect("the 8-bit value is inside a band");
            assert!(
                wide >= band.dmx_from && wide <= band.dmx_to,
                "{level} lands in the same band as 8-bit ({wide} vs {narrow})"
            );
        }
    }

    #[test]
    fn sixteen_bit_raw_round_trips_through_the_level() {
        let channel = Channel {
            fine_offset: Some(2),
            functions: vec![function(0.0, 100.0, 0, 255)],
            ..dimmer_channel(Vec::new())
        };
        for raw in [0u16, 1, 32767, 32768, 65535] {
            let level = channel.level_from_raw16(raw);
            assert_eq!(channel.raw16_from_level(level), raw, "raw {raw}");
        }
        assert_eq!(
            channel.level_from_raw(128),
            128.0 / 255.0,
            "8-bit stays exact"
        );
    }

    #[test]
    fn sets_are_addressable_by_index_and_exact_name() {
        let channel = dimmer_channel(vec![
            named_function("Gobo 1", 0.0, 10.0, 0, 15),
            named_function("Gobo 2", 10.0, 20.0, 16, 47),
            named_function("Rotate CW", 20.0, 30.0, 48, 95),
        ]);
        assert_eq!(channel.sets().count(), 3);
        assert_eq!(
            channel.level_for_set(SetSelector::Index(2)),
            Some(0.5),
            "the second set's physical midpoint"
        );
        assert_eq!(
            channel.level_for_set(SetSelector::Name("rotate cw")),
            channel.level_for_set(SetSelector::Index(3)),
            "names are case-insensitive"
        );
        assert_eq!(
            channel.level_for_set(SetSelector::Name("Gobo")),
            None,
            "no prefix matching"
        );
        assert_eq!(channel.level_for_set(SetSelector::Index(4)), None);
        for (index, function) in channel.sets() {
            let level = channel.level_for_set(SetSelector::Index(index)).unwrap();
            let raw = channel.raw_from_level(level);
            assert!(
                raw >= function.dmx_from && raw <= function.dmx_to,
                "set {index} lands in its band, got {raw}"
            );
        }
    }

    #[test]
    fn footprint_is_highest_offset() {
        let mode = Mode {
            name: "4ch".into(),
            channels: vec![
                Channel {
                    offset: 1,
                    fine_offset: None,
                    attribute: "Dimmer".into(),
                    default_raw: 0,
                    functions: vec![],
                },
                Channel {
                    offset: 4,
                    fine_offset: None,
                    attribute: "ColorAdd_B".into(),
                    default_raw: 0,
                    functions: vec![],
                },
            ],
        };
        assert_eq!(mode.footprint(), 4);
        let wide = Mode {
            name: "wide".into(),
            channels: vec![pan_channel()],
        };
        assert_eq!(wide.footprint(), 2, "the fine slot is inside the footprint");
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

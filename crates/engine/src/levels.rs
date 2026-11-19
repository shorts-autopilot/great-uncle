//! Normalised level maps.
//!
//! A [`Levels`] value is `fixture number → attribute → level (0..=1)`. The same
//! type carries a programmer state, a cue, a palette and the accumulated output
//! of a running playback, so every layer speaks in physical values and only the
//! renderer ever converts to DMX.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::fixture_type::DIMMER;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Levels {
    fixtures: BTreeMap<u16, BTreeMap<String, f64>>,
}

impl Levels {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no fixture holds any value.
    pub fn is_empty(&self) -> bool {
        self.fixtures.is_empty()
    }

    /// Number of stored fixture/attribute values.
    pub fn len(&self) -> usize {
        self.fixtures.values().map(BTreeMap::len).sum()
    }

    pub fn fixture_count(&self) -> usize {
        self.fixtures.len()
    }

    pub fn fixture(&self, number: u16) -> Option<&BTreeMap<String, f64>> {
        self.fixtures.get(&number)
    }

    pub fn fixtures(&self) -> impl Iterator<Item = (u16, &BTreeMap<String, f64>)> {
        self.fixtures
            .iter()
            .map(|(number, attributes)| (*number, attributes))
    }

    pub fn fixture_numbers(&self) -> impl Iterator<Item = u16> + '_ {
        self.fixtures.keys().copied()
    }

    /// Case-insensitive attribute lookup.
    pub fn get(&self, fixture: u16, attribute: &str) -> Option<f64> {
        self.fixtures
            .get(&fixture)?
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(attribute))
            .map(|(_, level)| *level)
    }

    /// Store a value, replacing an existing key that differs only in case.
    pub fn set(&mut self, fixture: u16, attribute: &str, level: f64) {
        let attributes = self.fixtures.entry(fixture).or_default();
        let key = attributes
            .keys()
            .find(|name| name.eq_ignore_ascii_case(attribute))
            .cloned();
        attributes.insert(key.unwrap_or_else(|| attribute.to_string()), level);
    }

    pub fn remove(&mut self, fixture: u16) {
        self.fixtures.remove(&fixture);
    }

    pub fn clear(&mut self) {
        self.fixtures.clear();
    }

    /// Replace every value of `other` on top of this state.
    pub fn overlay(&mut self, other: &Levels) {
        for (fixture, attributes) in &other.fixtures {
            for (attribute, level) in attributes {
                self.set(*fixture, attribute, *level);
            }
        }
    }

    /// Values that differ from `previous`: the tracked delta of a cue.
    pub fn difference(&self, previous: &Levels) -> Levels {
        let mut delta = Levels::new();
        for (fixture, attributes) in &self.fixtures {
            for (attribute, level) in attributes {
                if previous.get(*fixture, attribute) != Some(*level) {
                    delta.set(*fixture, attribute, *level);
                }
            }
        }
        delta
    }

    /// Interpolate between two states. An attribute missing on one side counts
    /// as 0, which is what a fade from a cue without that attribute needs.
    pub fn blend(from: &Levels, to: &Levels, fraction: f64) -> Levels {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut result = Levels::new();
        let mut fixtures: Vec<u16> = from
            .fixtures
            .keys()
            .chain(to.fixtures.keys())
            .copied()
            .collect();
        fixtures.sort_unstable();
        fixtures.dedup();

        for fixture in fixtures {
            let mut attributes: Vec<&String> = from
                .fixtures
                .get(&fixture)
                .into_iter()
                .flat_map(|map| map.keys())
                .chain(
                    to.fixtures
                        .get(&fixture)
                        .into_iter()
                        .flat_map(|map| map.keys()),
                )
                .collect();
            attributes.sort();
            attributes.dedup();
            for attribute in attributes {
                let start = from.get(fixture, attribute).unwrap_or(0.0);
                let end = to.get(fixture, attribute).unwrap_or(0.0);
                let value = if (start - end).abs() < 1e-12 {
                    end
                } else {
                    start + (end - start) * fraction
                };
                result.set(fixture, attribute, value.clamp(0.0, 1.0));
            }
        }
        result
    }

    /// Merge a second playback on top: intensity is highest-takes-precedence
    /// (HTP), every other attribute is latest-takes-precedence (LTP).
    pub fn merge_htp(&mut self, other: &Levels) {
        for (fixture, attributes) in &other.fixtures {
            for (attribute, level) in attributes {
                if attribute.eq_ignore_ascii_case(DIMMER) {
                    let current = self.get(*fixture, attribute).unwrap_or(0.0);
                    self.set(*fixture, attribute, current.max(*level));
                } else {
                    self.set(*fixture, attribute, *level);
                }
            }
        }
    }

    /// Scale intensity only. A fader at 50% dims a cue but does not desaturate
    /// its colours or move its positions.
    pub fn scale_intensity(&self, factor: f64) -> Levels {
        let factor = if factor.is_finite() {
            factor.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if (factor - 1.0).abs() < 1e-12 {
            return self.clone();
        }
        let mut result = Levels::new();
        for (fixture, attributes) in &self.fixtures {
            for (attribute, level) in attributes {
                let value = if attribute.eq_ignore_ascii_case(DIMMER) {
                    level * factor
                } else {
                    *level
                };
                result.set(*fixture, attribute, value);
            }
        }
        result
    }

    /// Every attribute name in use, useful for error messages and listings.
    pub fn attributes(&self) -> Vec<String> {
        let mut attributes: Vec<String> = self
            .fixtures
            .values()
            .flat_map(|map| map.keys().cloned())
            .collect();
        attributes.sort();
        attributes.dedup();
        attributes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(values: &[(u16, &str, f64)]) -> Levels {
        let mut levels = Levels::new();
        for (fixture, attribute, level) in values {
            levels.set(*fixture, attribute, *level);
        }
        levels
    }

    #[test]
    fn set_and_get_are_case_insensitive() {
        let mut levels = Levels::new();
        levels.set(1, "Dimmer", 0.5);
        assert_eq!(levels.get(1, "dimmer"), Some(0.5));
        levels.set(1, "DIMMER", 0.75);
        assert_eq!(levels.len(), 1, "the key is replaced, not duplicated");
        assert_eq!(levels.get(1, "Dimmer"), Some(0.75));
    }

    #[test]
    fn difference_keeps_only_changed_values() {
        let previous = levels(&[(1, "Dimmer", 0.5), (1, "ColorAdd_R", 1.0)]);
        let current = levels(&[(1, "Dimmer", 0.5), (1, "ColorAdd_R", 0.0)]);
        let delta = current.difference(&previous);
        assert_eq!(delta.get(1, "Dimmer"), None);
        assert_eq!(delta.get(1, "ColorAdd_R"), Some(0.0));
        assert_eq!(delta.len(), 1);
    }

    #[test]
    fn blend_interpolates_and_treats_missing_as_zero() {
        let from = levels(&[(1, "Dimmer", 1.0), (2, "Dimmer", 1.0)]);
        let to = levels(&[(1, "Dimmer", 0.0), (2, "ColorAdd_B", 1.0)]);
        let half = Levels::blend(&from, &to, 0.5);
        assert_eq!(half.get(1, "Dimmer"), Some(0.5));
        assert_eq!(half.get(2, "Dimmer"), Some(0.5), "fades to black");
        assert_eq!(half.get(2, "ColorAdd_B"), Some(0.5), "fades up from zero");
        let start = Levels::blend(&from, &to, 0.0);
        assert_eq!(start.get(1, "Dimmer"), Some(1.0));
        assert_eq!(start.get(2, "Dimmer"), Some(1.0));
        assert_eq!(start.get(2, "ColorAdd_B"), Some(0.0));
        let end = Levels::blend(&from, &to, 1.0);
        assert_eq!(end.get(1, "Dimmer"), Some(0.0));
        assert_eq!(end.get(2, "Dimmer"), Some(0.0));
        assert_eq!(end.get(2, "ColorAdd_B"), Some(1.0));
    }

    #[test]
    fn overlay_replaces_values_without_dropping_others() {
        let mut state = levels(&[(1, "Dimmer", 0.2), (1, "ColorAdd_R", 0.4)]);
        state.overlay(&levels(&[(1, "Dimmer", 0.9), (2, "Dimmer", 0.3)]));
        assert_eq!(state.get(1, "Dimmer"), Some(0.9));
        assert_eq!(state.get(1, "ColorAdd_R"), Some(0.4));
        assert_eq!(state.get(2, "Dimmer"), Some(0.3));
    }

    #[test]
    fn merge_htp_takes_the_highest_intensity_and_latest_colour() {
        let mut state = levels(&[(1, "Dimmer", 0.2), (1, "ColorAdd_R", 0.1)]);
        state.merge_htp(&levels(&[(1, "Dimmer", 0.6), (1, "ColorAdd_R", 0.9)]));
        assert_eq!(state.get(1, "Dimmer"), Some(0.6));
        assert_eq!(state.get(1, "ColorAdd_R"), Some(0.9));

        state.merge_htp(&levels(&[(1, "Dimmer", 0.4)]));
        assert_eq!(
            state.get(1, "Dimmer"),
            Some(0.6),
            "the lower intensity does not win"
        );
    }

    #[test]
    fn scale_intensity_leaves_other_attributes_alone() {
        let state = levels(&[(1, "Dimmer", 1.0), (1, "ColorAdd_R", 1.0)]);
        let scaled = state.scale_intensity(0.25);
        assert_eq!(scaled.get(1, "Dimmer"), Some(0.25));
        assert_eq!(scaled.get(1, "ColorAdd_R"), Some(1.0));
        assert_eq!(state.scale_intensity(f64::NAN).get(1, "Dimmer"), Some(0.0));
    }
}

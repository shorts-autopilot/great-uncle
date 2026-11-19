//! Typed palettes (preset pools).
//!
//! A palette stores one value per attribute per **fixture type**, grouped by a
//! feature pool (`Dimmer`, `Color`, `Position`, …). Grouping by type keeps a
//! palette honest: a colour stored on a wash is not silently applied to a
//! profile spot with a different colour system.
//!
//! Pool names come from the fixture type's own GDTF `Feature` (the part before
//! the first dot), falling back to the standard attribute name prefixes. They
//! are matched case-insensitively and never invented for a fixture type.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::fixture_type::FixtureType;
use crate::show::{Fixture, Show};

/// Pool used when nothing in a fixture type describes the attribute better.
pub const OTHER_POOL: &str = "Other";

/// Fixture type key → attribute → normalised level.
pub type PaletteValues = BTreeMap<String, BTreeMap<String, f64>>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    /// Feature pool, e.g. `Color`.
    pub feature: String,
    pub number: u16,
    #[serde(default)]
    pub name: String,
    /// Fixture type key → attribute → normalised level.
    #[serde(default)]
    pub values: PaletteValues,
}

impl Palette {
    /// `Color 1`, the identity used in listings and messages.
    pub fn label(&self) -> String {
        format!("{} {}", self.feature, self.number)
    }

    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            self.label()
        } else {
            format!("{} \"{}\"", self.label(), self.name)
        }
    }

    /// Values stored for one fixture type.
    pub fn values_for(&self, type_key: &str) -> Option<&BTreeMap<String, f64>> {
        self.values
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(type_key))
            .map(|(_, values)| values)
    }
}

/// Pool for one attribute of one fixture type.
pub fn pool_for(fixture_type: &FixtureType, attribute: &str) -> String {
    if let Some(definition) = fixture_type.attribute(attribute) {
        let feature = definition.feature.trim();
        if !feature.is_empty() {
            let head = feature.split(['.', ' ', '/']).next().unwrap_or(feature);
            return canonical_pool(head);
        }
    }
    canonical_pool(guess_pool(attribute))
}

/// Attribute name prefixes used when a profile has no feature metadata.
fn guess_pool(attribute: &str) -> &'static str {
    let lower = attribute.to_ascii_lowercase();
    if lower == "dimmer" || lower == "intensity" {
        "Dimmer"
    } else if lower.starts_with("color") || lower.starts_with("colour") {
        "Color"
    } else if lower.starts_with("pan") || lower.starts_with("tilt") {
        "Position"
    } else if lower.starts_with("gobo") {
        "Gobo"
    } else if lower.starts_with("zoom")
        || lower.starts_with("focus")
        || lower.starts_with("beam")
        || lower.starts_with("shutter")
        || lower.starts_with("iris")
        || lower.starts_with("prism")
        || lower.starts_with("frost")
        || lower.starts_with("strobe")
    {
        "Beam"
    } else {
        OTHER_POOL
    }
}

fn canonical_pool(name: &str) -> String {
    let trimmed = name.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "colour" => "Color".to_string(),
        "color" => "Color".to_string(),
        "dimmer" | "intensity" => "Dimmer".to_string(),
        "position" => "Position".to_string(),
        "beam" => "Beam".to_string(),
        "gobo" => "Gobo".to_string(),
        "focus" => "Focus".to_string(),
        "shapers" => "Shapers".to_string(),
        "control" => "Control".to_string(),
        "" => OTHER_POOL.to_string(),
        _ => trimmed.to_string(),
    }
}

/// Capture the values of one pool from the given fixtures.
///
/// Returns the canonical pool name and `type key → attribute → level`. Every
/// attribute of the pool that a selected mode contains is captured, including
/// attributes that are currently at 0: a colour palette stores all of RGB.
pub fn capture(
    show: &Show,
    targets: &[u16],
    feature: &str,
) -> Result<(String, PaletteValues), String> {
    let requested = feature.trim();
    if requested.is_empty() {
        return Err("a palette needs a feature pool, e.g. `Color`".to_string());
    }

    let mut pool_name: Option<String> = None;
    let mut values = PaletteValues::new();
    let mut available: BTreeSet<String> = BTreeSet::new();

    for number in targets {
        let Some(fixture) = show.fixture(*number) else {
            continue;
        };
        let (fixture_type, mode) = show
            .mode_for(fixture)
            .map_err(|error| format!("fixture {number}: {error}"))?;
        let type_key = fixture_type.key();
        for channel in &mode.channels {
            let pool = pool_for(fixture_type, &channel.attribute);
            if !pool.eq_ignore_ascii_case(requested) {
                available.insert(pool);
                continue;
            }
            pool_name.get_or_insert(pool);
            let level = fixture
                .levels
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&channel.attribute))
                .map(|(_, level)| *level)
                .unwrap_or(0.0);
            values
                .entry(type_key.clone())
                .or_default()
                .insert(channel.attribute.clone(), level);
        }
    }

    match pool_name {
        Some(pool) if !values.is_empty() => Ok((pool, values)),
        _ => {
            let hint = if available.is_empty() {
                String::new()
            } else {
                format!(
                    " (pools in the selection: {})",
                    available.into_iter().collect::<Vec<_>>().join(", ")
                )
            };
            Err(format!(
                "nothing in the selection has a `{requested}` attribute{hint}"
            ))
        }
    }
}

/// Values this palette gives one fixture, limited to attributes its mode has.
pub fn resolve(palette: &Palette, show: &Show, fixture: &Fixture) -> BTreeMap<String, f64> {
    let mut resolved = BTreeMap::new();
    let Ok((fixture_type, mode)) = show.mode_for(fixture) else {
        return resolved;
    };
    let Some(values) = palette.values_for(&fixture_type.key()) else {
        return resolved;
    };
    for (attribute, level) in values {
        if let Some(channel) = mode.channel_for(attribute) {
            resolved.insert(channel.attribute.clone(), *level);
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{demo_fixture_type, demo_show};

    fn sample_palette(values: &[(&str, &str, f64)]) -> Palette {
        let mut values_by_type: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
        for (type_key, attribute, level) in values {
            values_by_type
                .entry((*type_key).to_string())
                .or_default()
                .insert((*attribute).to_string(), *level);
        }
        Palette {
            feature: "Color".into(),
            number: 1,
            name: "Warm".into(),
            values: values_by_type,
        }
    }

    #[test]
    fn pools_come_from_the_profile_feature() {
        let fixture_type = demo_fixture_type();
        assert_eq!(pool_for(&fixture_type, "Dimmer"), "Dimmer");
        assert_eq!(pool_for(&fixture_type, "ColorAdd_R"), "Color");
    }

    #[test]
    fn unknown_attributes_fall_back_to_name_prefixes() {
        let mut fixture_type = demo_fixture_type();
        fixture_type.attributes.clear();
        assert_eq!(pool_for(&fixture_type, "Pan"), "Position");
        assert_eq!(pool_for(&fixture_type, "Zoom"), "Beam");
        assert_eq!(pool_for(&fixture_type, "Prism1"), "Beam");
        assert_eq!(pool_for(&fixture_type, "SomethingElse"), OTHER_POOL);
    }

    #[test]
    fn capture_reads_every_attribute_of_the_pool() {
        let mut show = demo_show();
        show.fixtures[0].levels.insert("ColorAdd_R".into(), 1.0);
        show.fixtures[0].levels.insert("ColorAdd_G".into(), 0.25);
        let (pool, values) = capture(&show, &[1], "color").unwrap();
        assert_eq!(pool, "Color");
        let stored = values.get("OpenLight Demo Wash").unwrap();
        assert_eq!(stored.get("ColorAdd_R"), Some(&1.0));
        assert_eq!(stored.get("ColorAdd_G"), Some(&0.25));
        assert_eq!(
            stored.get("ColorAdd_B"),
            Some(&0.0),
            "unset colour is stored as zero"
        );
        assert!(!stored.contains_key("Dimmer"));
    }

    #[test]
    fn capture_explains_when_nothing_matches() {
        let show = demo_show();
        let error = capture(&show, &[1], "Position").unwrap_err();
        assert!(error.contains("nothing in the selection"), "{error}");
        assert!(error.contains("Dimmer"), "{error}");
    }

    #[test]
    fn resolve_keeps_only_attributes_the_mode_has() {
        let show = demo_show();
        let palette = sample_palette(&[("OpenLight Demo Wash", "ColorAdd_R", 1.0)]);
        let values = resolve(&palette, &show, &show.fixtures[0]);
        assert_eq!(values.get("ColorAdd_R"), Some(&1.0));

        let foreign = sample_palette(&[("Other Fixture", "ColorAdd_R", 1.0)]);
        assert!(resolve(&foreign, &show, &show.fixtures[0]).is_empty());
    }
}

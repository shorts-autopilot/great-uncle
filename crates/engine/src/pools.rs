//! Groups, worlds and filters.
//!
//! * A **group** is a named, numbered set of fixtures with its own intensity
//!   master. Groups are how a rig is addressed in one word.
//! * A **world** limits which fixtures may be selected and programmed. It never
//!   limits output: a cue that is already running keeps driving the whole rig.
//! * A **filter** limits which features (attribute pools) may be programmed. It
//!   is applied to commands and to what a store captures.
//!
//! Worlds and filters refer to fixtures and pools by name/number rather than by
//! index, and they tolerate dangling fixture numbers, so unpatching a fixture
//! never makes a show unloadable.

use serde::{Deserialize, Serialize};

pub const MAX_POOL_OBJECTS: usize = 4096;
pub const MAX_GROUP_FIXTURES: usize = 4096;

fn full() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub number: u16,
    #[serde(default)]
    pub name: String,
    /// Fixture numbers. Numbers that are not patched are ignored when used.
    #[serde(default)]
    pub fixtures: Vec<u16>,
    /// Group intensity master, 0..=1.
    #[serde(default = "full")]
    pub master: f64,
}

impl Group {
    pub fn new(number: u16) -> Self {
        Self {
            number,
            name: String::new(),
            fixtures: Vec::new(),
            master: 1.0,
        }
    }

    pub fn contains(&self, fixture: u16) -> bool {
        self.fixtures.contains(&fixture)
    }

    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("Group {}", self.number)
        } else {
            format!("Group {} \"{}\"", self.number, self.name)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct World {
    pub number: u16,
    #[serde(default)]
    pub name: String,
    /// Fixture numbers this world allows. Same dangling-number tolerance as groups.
    #[serde(default)]
    pub fixtures: Vec<u16>,
}

impl World {
    pub fn new(number: u16) -> Self {
        Self {
            number,
            name: String::new(),
            fixtures: Vec::new(),
        }
    }

    pub fn allows(&self, fixture: u16) -> bool {
        self.fixtures.contains(&fixture)
    }

    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("World {}", self.number)
        } else {
            format!("World {} \"{}\"", self.number, self.name)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub number: u16,
    #[serde(default)]
    pub name: String,
    /// Feature pools this filter allows, e.g. `Dimmer` or `Color`.
    #[serde(default)]
    pub features: Vec<String>,
}

impl Filter {
    pub fn new(number: u16) -> Self {
        Self {
            number,
            name: String::new(),
            features: Vec::new(),
        }
    }

    /// Pool names are matched case-insensitively.
    pub fn allows(&self, pool: &str) -> bool {
        self.features
            .iter()
            .any(|feature| feature.trim().eq_ignore_ascii_case(pool.trim()))
    }

    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("Filter {}", self.number)
        } else {
            format!("Filter {} \"{}\"", self.number, self.name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_match_their_members() {
        let group = Group {
            number: 1,
            name: "Front".into(),
            fixtures: vec![1, 2, 3],
            master: 1.0,
        };
        assert!(group.contains(2));
        assert!(!group.contains(4));
        assert_eq!(group.display_name(), "Group 1 \"Front\"");
    }

    #[test]
    fn a_world_lists_the_fixtures_it_allows() {
        let world = World {
            number: 2,
            name: String::new(),
            fixtures: vec![4, 5],
        };
        assert!(world.allows(4));
        assert!(!world.allows(1));
        assert_eq!(world.display_name(), "World 2");
    }

    #[test]
    fn a_filter_matches_pools_case_insensitively() {
        let filter = Filter {
            number: 1,
            name: "Intensity".into(),
            features: vec!["Dimmer".into(), "Color".into()],
        };
        assert!(filter.allows("dimmer"));
        assert!(filter.allows(" color "));
        assert!(!filter.allows("Position"));
    }
}

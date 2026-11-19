//! Synthetic fixture data shared by engine tests. Not compiled into the library.

use std::collections::BTreeMap;

use crate::fixture_type::{AttributeDef, Channel, ChannelFunction, FixtureType, Mode};
use crate::show::{Fixture, Show};

/// A synthetic 4 channel wash. Not derived from any vendor fixture profile.
pub fn demo_fixture_type() -> FixtureType {
    let attribute = |name: &str, pretty: &str, feature: &str| AttributeDef {
        name: name.to_string(),
        pretty: pretty.to_string(),
        feature: feature.into(),
        unit: "Percent".into(),
    };
    let channel = |offset: u16, name: &str| Channel {
        offset,
        attribute: name.to_string(),
        default_raw: 0,
        functions: vec![ChannelFunction {
            name: name.to_string(),
            dmx_from: 0,
            dmx_to: 255,
            physical_from: 0.0,
            physical_to: 100.0,
        }],
    };

    FixtureType {
        manufacturer: "OpenLight".into(),
        name: "Demo Wash".into(),
        description: "Synthetic four channel wash used by tests and demos.".into(),
        attributes: vec![
            attribute("Dimmer", "Dimmer", "Dimmer"),
            attribute("ColorAdd_R", "Red", "Color.RGB"),
            attribute("ColorAdd_G", "Green", "Color.RGB"),
            attribute("ColorAdd_B", "Blue", "Color.RGB"),
        ],
        modes: vec![Mode {
            name: "4ch".into(),
            channels: vec![
                channel(1, "Dimmer"),
                channel(2, "ColorAdd_R"),
                channel(3, "ColorAdd_G"),
                channel(4, "ColorAdd_B"),
            ],
        }],
    }
}

pub fn demo_show() -> Show {
    let mut show = Show::new();
    show.fixture_types.push(demo_fixture_type());
    show.fixtures.push(Fixture {
        number: 1,
        name: "Demo Wash".into(),
        type_key: "OpenLight Demo Wash".into(),
        mode: "4ch".into(),
        universe: 1,
        address: 1,
        // The programmer starts empty; unprogrammed attributes render as 0.
        levels: BTreeMap::new(),
    });
    show
}

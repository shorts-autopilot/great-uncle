//! Synthetic fixture data shared by engine tests. Not compiled into the library.

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
        fine_offset: None,
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
        home: Default::default(),
    }
}

/// A synthetic moving light: `Pan` with a fine byte and a `Gobo` wheel with
/// three named sets. Not derived from any vendor fixture profile.
pub fn demo_moving_type() -> FixtureType {
    let function = |name: &str, dmx_from: u8, dmx_to: u8, from: f64, to: f64| ChannelFunction {
        name: name.to_string(),
        dmx_from,
        dmx_to,
        physical_from: from,
        physical_to: to,
    };
    let attribute = |name: &str, feature: &str| AttributeDef {
        name: name.to_string(),
        pretty: name.to_string(),
        feature: feature.into(),
        unit: "Percent".into(),
    };

    FixtureType {
        manufacturer: "OpenLight".into(),
        name: "Demo Moving".into(),
        description: "Synthetic moving light with a fine pan byte and a gobo wheel.".into(),
        attributes: vec![attribute("Pan", "Position"), attribute("Gobo", "Gobo")],
        modes: vec![Mode {
            name: "3ch".into(),
            channels: vec![
                Channel {
                    offset: 1,
                    fine_offset: Some(2),
                    attribute: "Pan".into(),
                    default_raw: 0,
                    functions: Vec::new(),
                },
                Channel {
                    offset: 3,
                    fine_offset: None,
                    attribute: "Gobo".into(),
                    default_raw: 0,
                    functions: vec![
                        function("Gobo 1", 0, 15, 0.0, 10.0),
                        function("Gobo 2", 16, 47, 10.0, 20.0),
                        function("Rotate CW", 48, 95, 20.0, 30.0),
                    ],
                },
            ],
        }],
        home: Default::default(),
    }
}

pub fn demo_moving_show() -> Show {
    let mut show = Show::new();
    let fixture_type = demo_moving_type();
    let type_key = fixture_type.key();
    show.fixture_types.push(fixture_type);
    show.fixtures.push(Fixture {
        number: 1,
        name: "Demo Moving".into(),
        type_key,
        mode: "3ch".into(),
        universe: 1,
        address: 1,
        home: Default::default(),
    });
    show
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
        home: Default::default(),
    });
    show
}

/// A synthetic subtractive wash: Dimmer and CMY. Not derived from any vendor
/// fixture profile.
pub fn demo_cmy_type() -> FixtureType {
    let attribute = |name: &str, pretty: &str| AttributeDef {
        name: name.to_string(),
        pretty: pretty.to_string(),
        feature: "Color.CMY".into(),
        unit: "Percent".into(),
    };
    let channel = |offset: u16, name: &str| Channel {
        offset,
        fine_offset: None,
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
        name: "Demo CMY".into(),
        description: "Synthetic subtractive wash used by colour tests.".into(),
        attributes: vec![
            AttributeDef {
                name: "Dimmer".into(),
                pretty: "Dimmer".into(),
                feature: "Dimmer".into(),
                unit: "Percent".into(),
            },
            attribute("ColorSub_C", "Cyan"),
            attribute("ColorSub_M", "Magenta"),
            attribute("ColorSub_Y", "Yellow"),
        ],
        modes: vec![Mode {
            name: "4ch".into(),
            channels: vec![
                channel(1, "Dimmer"),
                channel(2, "ColorSub_C"),
                channel(3, "ColorSub_M"),
                channel(4, "ColorSub_Y"),
            ],
        }],
        home: Default::default(),
    }
}

pub fn demo_cmy_show() -> Show {
    let mut show = Show::new();
    show.fixture_types.push(demo_cmy_type());
    show.fixtures.push(Fixture {
        number: 1,
        name: "Demo CMY".into(),
        type_key: "OpenLight Demo CMY".into(),
        mode: "4ch".into(),
        universe: 1,
        address: 1,
        home: Default::default(),
    });
    show
}

/// A synthetic profile with a five-set colour wheel. Not derived from any
/// vendor fixture profile.
pub fn demo_wheel_type() -> FixtureType {
    let function = |name: &str, dmx_from: u8, dmx_to: u8, from: f64, to: f64| ChannelFunction {
        name: name.to_string(),
        dmx_from,
        dmx_to,
        physical_from: from,
        physical_to: to,
    };

    FixtureType {
        manufacturer: "OpenLight".into(),
        name: "Demo Wheel".into(),
        description: "Synthetic colour wheel used by colour tests.".into(),
        attributes: vec![
            AttributeDef {
                name: "Dimmer".into(),
                pretty: "Dimmer".into(),
                feature: "Dimmer".into(),
                unit: "Percent".into(),
            },
            AttributeDef {
                name: "ColorWheel1".into(),
                pretty: "Colour".into(),
                feature: "Color.Color".into(),
                unit: "Percent".into(),
            },
        ],
        modes: vec![Mode {
            name: "2ch".into(),
            channels: vec![
                Channel {
                    offset: 1,
                    fine_offset: None,
                    attribute: "Dimmer".into(),
                    default_raw: 0,
                    functions: Vec::new(),
                },
                Channel {
                    offset: 2,
                    fine_offset: None,
                    attribute: "ColorWheel1".into(),
                    default_raw: 0,
                    functions: vec![
                        function("Open", 0, 31, 0.0, 20.0),
                        function("Red", 32, 63, 20.0, 40.0),
                        function("Green", 64, 95, 40.0, 60.0),
                        function("Blue", 96, 127, 60.0, 80.0),
                        function("CTO 3200K", 128, 159, 80.0, 100.0),
                    ],
                },
            ],
        }],
        home: Default::default(),
    }
}

pub fn demo_wheel_show() -> Show {
    let mut show = Show::new();
    show.fixture_types.push(demo_wheel_type());
    show.fixtures.push(Fixture {
        number: 1,
        name: "Demo Wheel".into(),
        type_key: "OpenLight Demo Wheel".into(),
        mode: "2ch".into(),
        universe: 1,
        address: 1,
        home: Default::default(),
    });
    show
}
